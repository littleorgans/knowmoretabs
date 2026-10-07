//! One tab a render opens: made, loaded, read back and closed, and the
//! only way to speak to its page.
//!
//! slice: content
//! why: A browser's socket can address any tab it has, and the owner's
//!      browser has the owner's tabs. So a page is spoken to only through a
//!      tab this run created: its own socket when the browser was launched,
//!      or the session attached to it on the browser's socket when it is
//!      the owner's, whose events no other tab hears. A tab is closed once,
//!      on the normal path where a failure to close says the browser is
//!      what failed, or else when it is dropped, so an error or a panic
//!      leaves no tab of ours behind.

use std::time::Instant;

use serde_json::Value;
use url::Url;

use super::{Browser, Failed, LOAD_WAIT, Lifecycle, Mode, PASSING, READ_WAIT, Read, text};
use crate::cdp::{self, Command, Session, Socket, Target};

/// One page this run opened. Closed once, by [`Tab::close`] or else when
/// dropped.
pub(super) struct Tab<'a> {
    browser: &'a Browser,
    target: Target,
    channel: Channel,
    closed: bool,
}

enum Channel {
    /// A launched tab: its private context, and its own socket once
    /// loading connects it.
    Own {
        context: String,
        socket: Option<Box<Socket>>,
    },
    /// An attached tab: the session this run attached to it.
    Session(Session),
}

impl<'a> Tab<'a> {
    /// Opens a blank tab of this run's own in `browser`: in a private
    /// context of its own when launched; hidden, with a session attached to
    /// it, when attached. Fails with why when the browser does not answer.
    pub(super) fn open(browser: &'a Browser) -> Result<Self, String> {
        let (target, channel) = match browser.mode {
            Mode::Launched(_) => {
                let reply = browser.call(&Command::CreateBrowserContext)?;
                let context = text(&reply, "browserContextId");
                match create(browser, Some(context.clone()), false) {
                    Ok(target) => (
                        target,
                        Channel::Own {
                            context,
                            socket: None,
                        },
                    ),
                    Err(why) => {
                        let _ = browser.call(&Command::DisposeBrowserContext { context });
                        return Err(why);
                    }
                }
            }
            Mode::Attached(_) => {
                let target = create(browser, None, true)?;
                let attach = Command::AttachToTarget {
                    target: target.clone(),
                };
                match browser.call(&attach).map(|reply| Session::of(&reply)) {
                    Ok(Some(session)) => (target, Channel::Session(session)),
                    failed => {
                        let _ = browser.call(&Command::CloseTarget { target });
                        return Err(failed.err().unwrap_or_else(|| unnamed(browser, "session")));
                    }
                }
            }
        };
        Ok(Self {
            browser,
            target,
            channel,
            closed: false,
        })
    }

    /// Loads `url` and reads the page back once it has loaded and the
    /// network is idle, or at the deadline when it has loaded but never
    /// gone idle.
    pub(super) fn load(&mut self, url: &Url) -> Result<Read, Failed> {
        let deadline = Instant::now() + LOAD_WAIT;
        self.call(&Command::PageEnable, deadline)?;
        self.call(&Command::LifecycleEvents, deadline)?;
        // What the blank tab reported is not the page's.
        self.forget_events();
        let deadline = Instant::now() + LOAD_WAIT;
        let navigated = self.call(
            &Command::Navigate {
                url: url.to_string(),
            },
            deadline,
        )?;
        if let Some(error) = navigated["errorText"].as_str().filter(|e| !e.is_empty()) {
            let name = error.trim_start_matches("net::");
            return Err(Failed {
                reason: name.to_owned(),
                passing: PASSING.contains(&name),
            });
        }
        let mut lifecycle =
            Lifecycle::new(text(&navigated, "frameId"), text(&navigated, "loaderId"));
        if !lifecycle.wait(|until| self.event(until), deadline)? {
            return Err(Failed::from("timeout".to_owned()));
        }
        let reply = self.call(&Command::Extract, Instant::now() + READ_WAIT)?;
        if reply.get("exceptionDetails").is_some() {
            return Err(Failed::stop("the page could not be read"));
        }
        serde_json::from_value(reply["result"]["value"].clone())
            .map_err(|_| Failed::stop("the page could not be read"))
    }

    /// Closes the tab, then a launched tab's context: a browser that does
    /// not answer says why.
    pub(super) fn close(mut self) -> Result<(), String> {
        self.shut()
    }

    /// Sends `command` to this tab's page, connecting a launched tab's own
    /// socket first when loading has not yet.
    fn call(&mut self, command: &Command, deadline: Instant) -> Result<Value, String> {
        match &mut self.channel {
            Channel::Own { socket, .. } => {
                let socket = match socket {
                    Some(socket) => socket,
                    None => socket.insert(Box::new(Socket::connect(
                        self.browser.port,
                        &self.target.route(),
                        deadline,
                    )?)),
                };
                socket.call(command, None, deadline)
            }
            Channel::Session(session) => {
                self.browser.socket().call(command, Some(session), deadline)
            }
        }
    }

    /// This tab's next lifecycle event, or `None` at `deadline`.
    fn event(&mut self, deadline: Instant) -> Result<Option<cdp::Lifecycle>, String> {
        match &mut self.channel {
            Channel::Own { socket, .. } => socket
                .as_mut()
                .map_or(Ok(None), |socket| socket.event(None, deadline)),
            Channel::Session(session) => self.browser.socket().event(Some(session), deadline),
        }
    }

    fn forget_events(&mut self) {
        match &mut self.channel {
            Channel::Own { socket, .. } => {
                if let Some(socket) = socket {
                    socket.forget_events();
                }
            }
            Channel::Session(_) => self.browser.socket().forget_events(),
        }
    }

    /// Closes what this tab opened, once: its own socket first, then the
    /// tab, then a launched tab's context.
    fn shut(&mut self) -> Result<(), String> {
        if std::mem::replace(&mut self.closed, true) {
            return Ok(());
        }
        let context = match &mut self.channel {
            Channel::Own { context, socket } => {
                *socket = None;
                Some(context.clone())
            }
            Channel::Session(_) => None,
        };
        self.browser.call(&Command::CloseTarget {
            target: self.target.clone(),
        })?;
        if let Some(context) = context {
            self.browser
                .call(&Command::DisposeBrowserContext { context })?;
        }
        Ok(())
    }
}

/// A tab left open by an error or a panic is closed on the way out.
impl Drop for Tab<'_> {
    fn drop(&mut self) {
        let _ = self.shut();
    }
}

/// A blank tab in `browser`, in `context` when there is one, `hidden` from
/// the tab strip when asked.
fn create(browser: &Browser, context: Option<String>, hidden: bool) -> Result<Target, String> {
    let reply = browser.call(&Command::CreateTarget { context, hidden })?;
    Target::of(&reply).ok_or_else(|| unnamed(browser, "tab"))
}

/// Why a reply that should have named a `thing` did not.
fn unnamed(browser: &Browser, thing: &str) -> String {
    format!("{} named no {thing}", browser.name())
}
