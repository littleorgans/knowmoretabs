set positional-arguments

lib_port := "41878"
tagger_port := "41879"

check:
    cargo fmt --all -- --check
    python3 scripts/clippy.py
    cargo run --locked --package xtask -- slices --check
    uv run --locked --project lab/tagger ruff check lab/tagger scripts
    uv run --locked --project lab/tagger ruff format --check lab/tagger scripts
    @echo 'Python type checker: none configured'

test:
    cargo test --workspace --all-targets --locked
    node --test tests/serve_frontend.cjs tests/extension.cjs
    # As CI's lab job: the shell and browser tests run behind this checkout's debug build.
    cargo build --locked
    KMT_BIN="${CARGO_TARGET_DIR:-$PWD/target}/debug/knowmoretabs" uv run --locked --project lab/tagger pytest -q lab/tagger/tests

install:
    #!/usr/bin/env sh
    set -eu
    before="$(python3 -I scripts/installation.py begin)"
    # Make the destination explicit so the receipt can verify PATH reaches this install.
    install_root="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}"
    cargo install --path . --locked --root "$install_root"
    constraints="$(mktemp)"
    trap 'rm -f -- "$constraints"' EXIT
    uv export --locked --project lab/tagger --no-dev --no-emit-project --no-hashes --output-file "$constraints" > /dev/null
    uv tool install --force --constraints "$constraints" ./lab/tagger
    # The receipt `just up` checks; only a complete install writes one.
    python3 -I scripts/installation.py record --before "$before" --installed-kmt "$install_root/bin/knowmoretabs" --kmt "$(command -v knowmoretabs)" --tagger "$(command -v tagger)"

tagger *args:
    uv run --locked --project lab/tagger tagger app "$@"

# Start both apps; `just up dev` runs from this checkout.
up mode="": (_apps "up" mode lib_port tagger_port)

# Stop apps started by up and remove their logs and key.
down: (_apps "down" "" lib_port tagger_port)

[private]
_apps action mode library_port tagging_port:
    #!/usr/bin/env bash
    set -euo pipefail
    action="$1"; mode="$2"; ports=("$3" "$4")
    [[ "$mode" == "" || "$mode" == dev ]] || { echo 'usage: just up [dev]' >&2; exit 1; }
    kmt=knowmoretabs; tagger=tagger
    if [[ "$action" == up && "$mode" == "" ]]; then
        # Launch exactly the pair the install receipt vouches for, before anything else happens.
        kmt="$(command -v knowmoretabs || true)"; tagger="$(command -v tagger || true)"
        python3 -I scripts/installation.py check --kmt "$kmt" --tagger "$tagger" || exit 1
    fi
    # One archive for both apps, so the Library's search ranks over its own pages.
    root="${KMT_ROOT:-$HOME/.knowmoretabs}"
    logs="$root/logs"
    # The launch's key: the Library proves each request to the Tagger with it, and the Tagger each reply. Made
    # once, kept while it exists, removed by down (lab/tagger/tagger/app/service_auth.py).
    key="$logs/session.key"
    status=0
    listeners() { lsof -nP -t -iTCP:"$1" -sTCP:LISTEN || [[ $? == 1 ]]; }
    live() { kill -0 "$1" 2>/dev/null; }
    # Build in the foreground so the startup wait below never covers compilation.
    [[ "$mode" != dev ]] || cargo build --locked
    # Add link and Send run the knowmoretabs this launch serves the Library with: the one the receipt vouches
    # for, or this checkout's debug build.
    [[ "$mode" != dev ]] || kmt="${CARGO_TARGET_DIR:-$PWD/target}/debug/knowmoretabs"
    [[ "$action" != up ]] || python3 -I lab/tagger/tagger/app/service_auth.py ensure "$key"
    # Separate process groups let down stop uv and the app's worker processes together.
    set -m
    for i in 0 1; do
        port="${ports[$i]}"
        if [[ "$i" == 0 ]]; then
            app=library; ours='* serve --port *'
            args=(--root "$root" serve --port "$port" --model-port "${ports[1]}" --model-secret-file "$key")
            command=("$kmt" "${args[@]}")
            [[ "$mode" != dev ]] || command=(cargo run --locked -- "${args[@]}")
        else
            app=tagger; ours='*tagger app --port *'
            args=(app --port "$port" --root "$root" --secret-file "$key" --kmt-bin "$kmt")
            command=("$tagger" "${args[@]}")
            [[ "$mode" != dev ]] || command=(uv run --locked --project lab/tagger tagger "${args[@]}")
        fi
        file="$logs/$app.pid"; log="$logs/$app.log"
        pid=""; [[ ! -f "$file" ]] || pid="$(cat "$file")"
        if [[ "$action" == up ]]; then
            if live "$pid"; then
                echo "$app already running (pid $pid)"
                continue
            fi
            holders="$(listeners "$port")"
            if [[ -n "$holders" ]]; then
                echo "$app port $port held by pid(s) $holders; skipping"
                continue
            fi
            mkdir -p "$logs"
            nohup "${command[@]}" > "$log" 2>&1 < /dev/null &
            pid=$!
            echo "$pid" > "$file"
            ready=false
            for ((attempt=0; attempt<120; attempt++)); do
                if ! live "$pid"; then break; fi
                if [[ -n "$(listeners "$port")" ]]; then ready=true; break; fi
                sleep 1
            done
            if [[ "$ready" == true ]]; then
                echo "$app started (pid $pid)"
            else
                echo "$app died or did not listen within 120s; log tail:" >&2
                tail -n 20 "$log" >&2
                status=1
            fi
        else
            if ! live "$pid"; then
                echo "$app nothing to stop"
            elif [[ "$(ps -p "$pid" -o command=)" != $ours ]]; then
                echo "$app pid $pid belongs to another process; leaving it alone"
            else
                kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid"
                for ((attempt=0; attempt<50; attempt++)); do
                    live "$pid" || break
                    sleep 0.1
                done
                if live "$pid"; then
                    echo "$app pid $pid still running; keeping pid and log" >&2
                    status=1
                    continue
                fi
                echo "$app stopped"
            fi
            rm -f -- "$file" "$log"
            holders="$(listeners "$port")"
            [[ -z "$holders" ]] || echo "$app port $port still held by pid(s) $holders; leaving them alone"
        fi
    done
    if [[ "$action" == up ]]; then
        # One address: the Library serves Tag and Add link too.
        echo "Library: http://127.0.0.1:${ports[0]}/"
    else
        # Both apps are stopped, so the key goes; a failed stop keeps it for the app still running.
        [[ "$status" != 0 ]] || rm -f -- "$key"
        [[ ! -d "$logs" ]] || rmdir "$logs" 2>/dev/null || true
    fi
    exit "$status"
