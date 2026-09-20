#!/usr/bin/env python3
"""Save Chrome's latest disk session to a dated archive and update its index."""
import argparse
from collections import Counter
from datetime import datetime
import fcntl
import hashlib
import html
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from urllib.parse import quote, urlsplit


def read_session(path):
    data = path.read_bytes()
    if data[:8] != b'SNSS\x03\x00\x00\x00':
        raise ValueError('Expected an unencrypted Chrome session in SNSS version 3 format')
    tabs, windows, closed_windows = {}, {}, set()
    counts = Counter()

    def tab(identifier):
        return tabs.setdefault(identifier, {'navigation': {}, 'selected': 0, 'pinned': False})

    offset = 8
    while offset < len(data):
        if len(data) - offset < 2:
            raise ValueError(f'Incomplete command length at byte {offset}')
        size = struct.unpack_from('<H', data, offset)[0]
        if size < 1 or offset + 2 + size > len(data):
            raise ValueError(f'Incomplete command at byte {offset}')
        command = data[offset + 2]
        payload = data[offset + 3:offset + 2 + size]
        offset += 2 + size
        counts[command] += 1
        if command == 0:
            window, identifier = struct.unpack('<ii', payload)
            tab(identifier)['window_id'] = window
        elif command in (2, 7):
            identifier, index = struct.unpack('<ii', payload)
            tab(identifier)['position' if command == 2 else 'selected'] = index
        elif command == 9:
            window, kind = struct.unpack('<ii', payload)
            windows[window] = kind
        elif command == 12:
            identifier = struct.unpack_from('<i', payload)[0]
            tab(identifier)['pinned'] = bool(payload[4])
        elif command == 16:
            tabs.pop(struct.unpack_from('<i', payload)[0], None)
        elif command == 17:
            closed_windows.add(struct.unpack_from('<i', payload)[0])
        elif command == 6:
            pickle_size, identifier, index, length = struct.unpack_from('<iiii', payload)
            if pickle_size != len(payload) - 4 or length < 0:
                raise ValueError('Invalid navigation record')
            url = payload[16:16 + length].decode('utf-8')
            title_offset = 16 + (length + 3) // 4 * 4
            title_length = struct.unpack_from('<i', payload, title_offset)[0]
            title_end = title_offset + 4 + title_length * 2
            if title_length < 0 or title_end > len(payload):
                raise ValueError('Invalid navigation title')
            title = payload[title_offset + 4:title_end].decode('utf-16-le')
            tab(identifier)['navigation'][index] = {'url': url, 'title': title}
        elif command in (5, 11, 24):
            identifier, start = struct.unpack_from('<ii', payload)
            state = tab(identifier)
            if command == 5:
                state['navigation'] = {i: n for i, n in state['navigation'].items() if i < start}
                continue
            count = start if command == 11 else struct.unpack_from('<i', payload, 8)[0]
            start = 0 if command == 11 else start
            end = start + count
            selected = state['selected']
            if start <= selected < end:
                state['selected'] = start - 1
            elif selected >= end:
                state['selected'] -= count
            state['navigation'] = {
                i if i < start else i - count: n
                for i, n in state['navigation'].items() if not start <= i < end
            }
        elif command not in {8, 13, 14, 19, 20, 21, 23, 25, 27, 28, 29, 30,
                             31, 32, 33, 34, 35, 36, 37, 255}:
            raise ValueError(f'Unsupported session command {command}; source preserved')
    if counts[255] != 1:
        raise ValueError('Missing or unexpected initial session marker')
    rows = []
    for identifier, state in tabs.items():
        window = state.get('window_id')
        if window in closed_windows:
            continue
        if window not in windows or 'position' not in state:
            raise ValueError(f'Tab {identifier} has incomplete window metadata')
        if state['selected'] not in state['navigation']:
            raise ValueError(f'Tab {identifier} has no navigation at its selected index')
        current = state['navigation'][state['selected']]
        domain = urlsplit(current['url']).netloc or urlsplit(current['url']).scheme or 'Other'
        rows.append(dict(current, tab_id=identifier, window_id=window,
                         position=state['position'], domain=domain, pinned=state['pinned']))
    rows.sort(key=lambda row: (row['window_id'], row['position'], row['tab_id']))
    numbers = {identifier: index + 1 for index, identifier in enumerate(sorted({r['window_id'] for r in rows}))}
    for row in rows:
        row['window'] = numbers[row['window_id']]
    return rows, {'source': str(path), 'sha256': hashlib.sha256(data).hexdigest(),
                  'commands': sum(counts.values()), 'selected_urls_verified': len(rows),
                  'saved_at': datetime.fromtimestamp(path.stat().st_mtime).isoformat(timespec='seconds')}


def markdown(rows, metadata):
    lines = ['# Saved Chrome tabs', '', f"Saved session: {metadata['saved_at']}", '',
             f"{len(rows)} tabs across {len({r['window'] for r in rows})} windows.", '']
    previous = None
    for row in rows:
        if row['window'] != previous:
            previous = row['window']
            lines.extend([f'## Window {previous}', ''])
        title = ' '.join((row['title'] or row['url']).split())
        for character in ('\\', '[', ']', '*', '_', '`'):
            title = title.replace(character, '\\' + character)
        url = row['url'].replace('<', '%3C').replace('>', '%3E').replace('\n', '%0A')
        lines.extend([f"- [{title}](<{url}>){' · Pinned' if row['pinned'] else ''}", ''])
    return '\n'.join(lines)


def write_export(rows, metadata, destination):
    (destination / 'tabs.md').write_text(markdown(rows, metadata), encoding='utf-8')
    (destination / 'tabs.json').write_text(
        json.dumps({'metadata': metadata, 'tabs': rows}, indent=2, ensure_ascii=False),
        encoding='utf-8',
    )
    page = fill(HTML_TEMPLATE, SAVED_TAB_DATA=embed_json({'metadata': metadata, 'tabs': rows}))
    (destination / 'tabs.html').write_text(page, encoding='utf-8')


def embed_json(data):
    return json.dumps(data, ensure_ascii=True).replace('<', '\\u003c')


def fill(template, **parts):
    for name, value in parts.items():
        template = template.replace(f'/* {name} */', value)
    return template


def choose_session(chrome_root, profile=None, session=None):
    if session is not None:
        source = session.expanduser().resolve(strict=True)
        if not source.is_file():
            raise ValueError(f'Expected a session file: {source}')
        return source, None
    if profile is None:
        state = json.loads((chrome_root / 'Local State').read_text(encoding='utf-8'))
        profile = state.get('profile', {}).get('last_used', 'Default')
    if not isinstance(profile, str) or not profile or Path(profile).name != profile or profile in ('.', '..'):
        raise ValueError('Profile must be a Chrome profile directory name, such as Default')
    candidates = [path for path in (chrome_root / profile / 'Sessions').glob('Session_*')
                  if path.is_file() and path.name.removeprefix('Session_').isdigit()]
    if not candidates:
        raise ValueError(f'No saved Chrome sessions found for profile {profile}')
    return max(candidates, key=lambda path: int(path.name.removeprefix('Session_'))), profile


def copy_stable_session(source, destination):
    for _ in range(3):
        with source.open('rb') as stream:
            before = os.fstat(stream.fileno())
            data = stream.read()
            after = os.fstat(stream.fileno())
        current = source.stat()
        signatures = {(s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)
                      for s in (before, after, current)}
        if len(signatures) == 1 and len(data) == after.st_size:
            destination.parent.mkdir()
            destination.write_bytes(data)
            os.utime(destination, ns=(after.st_atime_ns, after.st_mtime_ns))
            return
    raise ValueError('Chrome changed its session during capture. Run the command again.')


def layout(rows):
    return [(row['window'], row['position'], row['url'], row['pinned']) for row in rows]


def publish_snapshot(root, source, profile, now, previous=None):
    """Publish a snapshot, unless its layout matches the previous rows."""
    with tempfile.TemporaryDirectory(prefix='.pending-', dir=root) as temporary:
        staging = Path(temporary)
        backup = staging / 'session-backup' / source.name
        copy_stable_session(source, backup)
        rows, metadata = read_session(backup)
        if previous is not None and layout(rows) == layout(previous):
            return None, rows, metadata
        metadata.update(source=str(source), snapshot=str(backup.relative_to(staging)),
                        profile=profile, exported_at=now.isoformat(timespec='seconds'))
        write_export(rows, metadata, staging)
        name = now.strftime('%Y-%m-%d-%H%M%S')
        destination = root / name
        suffix = 2
        while destination.exists():
            destination = root / f'{name}-{suffix}'
            suffix += 1
        staging.rename(destination)
    return destination, rows, metadata


def load_archives(root):
    """Every published snapshot with its rows, newest first."""
    archives = []
    for path in root.rglob('tabs.json'):
        relative = path.parent.relative_to(root)
        if any(part.startswith('.') for part in relative.parts):
            continue
        if not all((path.parent / name).is_file() for name in ('tabs.html', 'tabs.md')):
            continue
        archive = json.loads(path.read_text(encoding='utf-8'))
        metadata = archive['metadata']
        exported = metadata.get('exported_at')
        if not exported:
            try:
                exported = datetime.strptime(relative.name, '%Y-%m-%d-%H%M%S').isoformat()
            except ValueError:
                exported = metadata['saved_at']
        archives.append({'path': relative.as_posix(), 'date': exported,
                         'metadata': metadata, 'tabs': archive['tabs']})
    return sorted(archives, key=lambda item: (item['date'][:19], item['path']), reverse=True)


def index_records(archives):
    return [{'path': archive['path'], 'base': quote(archive['path'], safe='/'), 'date': archive['date'],
             'saved': archive['metadata']['saved_at'], 'tabs': len(archive['tabs']),
             'windows': len({row['window'] for row in archive['tabs']}),
             'profile': archive['metadata'].get('profile') or 'Imported archive'}
            for archive in archives]


def is_local(url):
    parts = urlsplit(url)
    return parts.scheme == 'file' or parts.hostname in ('localhost', '127.0.0.1')


def library_records(archives, forgotten):
    """One record per remembered, non-local URL with its latest title and every snapshot, window, and tab it appeared in."""
    pages = {}
    for number, archive in enumerate(archives):
        for row in archive['tabs']:
            if row['url'] in forgotten or is_local(row['url']):
                continue
            page = pages.setdefault(row['url'], {'url': row['url'], 'title': row['title'],
                                                 'domain': row['domain'], 'seen': []})
            if not page['seen'] or page['seen'][-1][0] != number:
                page['seen'].append([number, []])
            page['seen'][-1][1].append([row['window'], row['tab_id']])
    return list(pages.values())


def render_index(archives, forgotten):
    escape = html.escape
    records = index_records(archives)
    entries = []
    for record in records:
        base = record['base']
        date = escape(record['date'].replace('T', ' at '))
        label = escape(record['path'])
        entries.append(
            f'<li class="archive"><div><a class="title" href="{base}/tabs.html">{date}</a>'
            f'<div class="url">{label} · {escape(record["profile"])}</div>'
            f'<div class="url">Session saved {escape(record["saved"].replace("T", " at "))}</div></div>'
            f'<div class="archive-meta"><span>{record["tabs"]} tabs · {record["windows"]} windows</span>'
            f'<div><a href="{base}/tabs.md" download>Markdown</a> '
            f'<a href="{base}/tabs.json" download>JSON</a></div></div></li>'
        )
    latest = f'<a class="latest" href="{records[0]["base"]}/tabs.html">Open latest snapshot →</a>' if records else ''
    listing = '<ol>' + ''.join(entries) + '</ol>' if records else '<p>No snapshots yet. Run <code>b_tabs_export</code> to save one.</p>'
    return fill(INDEX_TEMPLATE, LATEST=latest, SNAPSHOTS=listing,
                LIBRARY_DATA=embed_json({'snapshots': records, 'forgotten': len(forgotten),
                                         'pages': library_records(archives, forgotten)}))


def write_atomically(path, content):
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', prefix=f'.{path.stem}-', dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


def read_forgotten(root):
    """URLs hidden from the library, mapped to when they were forgotten. Snapshots still hold them."""
    path = root / 'forgotten.json'
    return json.loads(path.read_text(encoding='utf-8')) if path.exists() else {}


def update_forgotten(root, urls, restore, now):
    forgotten = read_forgotten(root)
    known = forgotten if restore else {row['url'] for archive in load_archives(root) for row in archive['tabs']}
    unknown = [url for url in urls if url not in known]
    if unknown:
        raise ValueError(('Not forgotten: ' if restore else 'Not in your library: ') + ', '.join(unknown))
    for url in urls:
        if restore:
            del forgotten[url]
        else:
            forgotten.setdefault(url, now.isoformat(timespec='seconds'))
    write_atomically(root / 'forgotten.json', json.dumps(forgotten, indent=2, ensure_ascii=False))


def update_index(root):
    write_atomically(root / 'index.html', render_index(load_archives(root), read_forgotten(root)))


def main():
    parser = argparse.ArgumentParser(description=__doc__, epilog=(
        'Reads the latest session saved to disk, which may lag behind live tabs. '
        'The --open option opens the index in your default browser. '
        'Each export gets a new directory; previous snapshots are never overwritten. '
        'A session whose windows and tabs match the previous snapshot is skipped.'
    ))
    inputs = parser.add_mutually_exclusive_group()
    inputs.add_argument('--session', type=Path, help='export a specific Session_* file')
    inputs.add_argument('--profile', help='Chrome profile directory, such as Default; defaults to last used')
    inputs.add_argument('--index-only', action='store_true', help='rebuild the index without creating a snapshot')
    inputs.add_argument('--forget', nargs='+', metavar='URL', help='hide pages from the library; snapshots keep them')
    inputs.add_argument('--restore', nargs='+', metavar='URL', help='bring forgotten pages back to the library')
    parser.add_argument('--output-root', type=Path, default=Path.home() / 'Documents/chrome-tabs',
                        help='archive root (default: ~/Documents/chrome-tabs)')
    parser.add_argument('--force', action='store_true', help='save a snapshot even when nothing changed')
    parser.add_argument('--open', action='store_true', help='open the index in your default browser afterwards')
    args = parser.parse_args()
    root = args.output_root.expanduser().resolve()
    destination = None
    try:
        source, profile = (None, None) if args.index_only or args.forget or args.restore else choose_session(
            Path.home() / 'Library/Application Support/Google/Chrome', args.profile, args.session)
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        with (root / '.export.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if source is not None:
                previous = None if args.force else next(
                    (archive for archive in load_archives(root)
                     if archive['metadata'].get('profile') == profile), None)
                destination, rows, metadata = publish_snapshot(
                    root, source, profile, datetime.now().astimezone(), previous and previous['tabs'])
                summary = f'{len(rows)} tabs across {len({row["window"] for row in rows})} windows'
                print(f'Source: {source} (saved {metadata["saved_at"]})')
                if destination is None:
                    print(f'No change since {previous["path"]}: {summary}. Nothing saved; use --force to save anyway.')
                else:
                    print(f'Saved {summary}.')
                    print(f'Snapshot: {destination / "tabs.html"}')
            if args.forget or args.restore:
                update_forgotten(root, args.forget or args.restore, bool(args.restore), datetime.now().astimezone())
                print(f'{"Restored" if args.restore else "Forgot"}: {len(args.forget or args.restore)}.')
            update_index(root)
        print(f'Index: {(root / "index.html").as_uri()}')
        if args.open:
            subprocess.run(['open', str(root / 'index.html')], check=True)
    except (OSError, ValueError, KeyError, TypeError, struct.error, subprocess.CalledProcessError) as error:
        print(f'b_tabs_export: {error}', file=sys.stderr)
        if destination is not None:
            print(f'Your snapshot is safe at {destination}. Rebuild its index with --index-only.', file=sys.stderr)
        return 1
    return 0


SHARED_STYLES = r""":root{color-scheme:light;--paper:#f6f4ef;--ink:#242824;--muted:#666b63;--line:#d8dcd2;--accent:#385d45}
*{box-sizing:border-box}body{margin:0;background:var(--paper);color:var(--ink);font:16px/1.5 'Avenir Next',Avenir,sans-serif}
main{max-width:1160px;margin:auto;padding:48px 32px 80px}.eyebrow{font-size:12px;text-transform:uppercase;letter-spacing:.16em;color:var(--accent);font-weight:700}
header{border-bottom:1px solid var(--line);padding-bottom:30px}h1{font:normal clamp(44px,7vw,76px)/1.1 Georgia,serif;letter-spacing:-.05em;margin:18px 0}header p{max-width:640px;color:var(--muted);margin:10px 0}
.stats{display:flex;gap:38px;margin-top:28px;flex-wrap:wrap}.stats strong{display:block;font:32px Georgia,serif}.stats span{font-size:12px;text-transform:uppercase;letter-spacing:.09em;color:var(--muted)}
.tools{display:grid;grid-template-columns:1fr 180px 220px;gap:16px;margin:30px 0 12px}label{display:flex;flex-direction:column;gap:7px;font-size:12px;font-weight:600;color:var(--muted)}
input,select,button{font:inherit}input,select{background:#fffefa;border:1px solid #a7afa1;padding:12px;color:var(--ink);border-radius:4px;width:100%;min-width:0;font-size:15px}
input:focus,select:focus,button:focus-visible,a:focus-visible{outline:3px solid #8db397;outline-offset:3px}
.utility{display:flex;justify-content:space-between;align-items:center;gap:16px;flex-wrap:wrap;margin-bottom:30px;color:var(--muted);font-size:13px}.utility a{color:var(--accent);margin-right:18px}.utility button{background:none;border:0;text-decoration:underline;color:var(--accent);padding:8px;cursor:pointer}
section{margin:26px 0 36px}h2{font:24px Georgia,serif;margin:0 0 10px;display:flex;gap:12px;align-items:baseline}h2 small{font:12px 'Avenir Next',sans-serif;color:var(--muted)}
ol{list-style:none;margin:0;padding:0;border-top:1px solid var(--line)}li{display:grid;grid-template-columns:42px 1fr;gap:8px;padding:13px 8px;border-bottom:1px solid var(--line)}li:hover{background:#eceedf}
.number{font:12px/26px Menlo,monospace;color:var(--muted)}.title{color:var(--ink);font-weight:600;text-decoration:none;overflow-wrap:anywhere}.title:hover{text-decoration:underline}.url{font:12px/1.6 Menlo,monospace;color:var(--muted);overflow-wrap:anywhere;margin-top:3px}li.target{background:#e3ead9;box-shadow:inset 3px 0 var(--accent)}.note{font-size:11px;color:var(--accent);margin-left:10px;white-space:nowrap}
#empty{padding:40px 0;color:var(--muted)}footer{border-top:1px solid var(--line);padding-top:20px;font-size:12px;color:var(--muted)}
@media(max-width:700px){main{padding:28px 18px 50px}.tools{grid-template-columns:1fr 1fr}.tools label:first-child{grid-column:1/-1}.stats{gap:24px}li{grid-template-columns:30px 1fr}.url{font-size:11px}}
"""

SHARED_SCRIPT = r"""const $=id=>document.getElementById(id);
function element(tag,text,className){const node=document.createElement(tag);if(text!==undefined)node.textContent=text;if(className)node.className=className;return node}
function pageLink(page){const safe=/^https?:\/\//i.test(page.url);const title=element(safe?'a':'span',page.title||page.url,'title');if(safe){title.href=page.url;title.target='_blank';title.rel='noopener noreferrer'}return title}
"""

HTML_TEMPLATE = fill(r"""<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Saved tabs · Chrome library</title>
<style>/* SHARED_STYLES */</style>
<main>
<header><a href="../index.html" class="eyebrow">← All snapshots</a><h1>A place for every tab.</h1>
<p>Your saved pages, ready when you are. Search the collection or browse by window and website. Links open individually.</p>
<p id="date"></p><div class="stats" id="stats"></div></header>
<div class="tools">
<label>Search your tabs<input id="search" type="search" placeholder="Title, website, or URL" autocomplete="off"></label>
<label>Organize by<select id="group"><option value="window">Window</option><option value="domain">Website</option></select></label>
<label>Website<select id="domain"><option value="">All websites</option></select></label>
</div>
<div class="utility"><span id="count" role="status" aria-live="polite"></span><div><a href="tabs.md" download>Markdown</a><a href="tabs.json" download>JSON</a><button id="reset" type="button">Clear filters</button></div></div>
<div id="results"></div><p id="empty" hidden>No tabs match this search. Try another title or clear the filters.</p>
<noscript>Enable JavaScript to search this archive, or open the Markdown copy linked above.</noscript>
<footer>Stored locally. No accounts, remote fonts, images, or background page loading. Duplicate tabs are preserved. Window numbers are assigned for this archive.</footer>
</main>
<script id="data" type="application/json">/* SAVED_TAB_DATA */</script>
<script>
/* SHARED_SCRIPT */const archive=JSON.parse(document.getElementById('data').textContent);
const tabs=archive.tabs;
$('date').textContent='Session saved '+archive.metadata.saved_at.replace('T',' at ')+'.';
const domains=[...new Set(tabs.map(t=>t.domain))].sort();
for(const [number,label] of [[tabs.length,'Tabs'],[new Set(tabs.map(t=>t.window)).size,'Windows'],[domains.length,'Websites']]){const stat=element('div');stat.append(element('strong',number),element('span',label));$('stats').append(stat)}
for(const domain of domains){const option=element('option',domain+' · '+tabs.filter(t=>t.domain===domain).length);option.value=domain;$('domain').append(option)}
function render(){
 const query=$('search').value.toLocaleLowerCase().trim();
 const selected=$('domain').value;
 const shown=tabs.filter(t=>(!selected||t.domain===selected)&&(!query||(t.title+' '+t.url).toLocaleLowerCase().includes(query)));
 $('count').textContent=shown.length+' of '+tabs.length+' tabs';$('empty').hidden=shown.length>0;
 const groups=new Map();const mode=$('group').value;
 for(const tab of shown){const key=mode==='window'?tab.window:tab.domain;if(!groups.has(key))groups.set(key,[]);groups.get(key).push(tab)}
 const fragment=document.createDocumentFragment();
 const keys=[...groups.keys()].sort((a,b)=>mode==='window'?a-b:a.localeCompare(b));
 for(const key of keys){
  const entries=groups.get(key);const section=element('section');const heading=element('h2',mode==='window'?'Window '+key:key);heading.append(element('small',entries.length+' '+(entries.length===1?'tab':'tabs')));section.append(heading);
  const list=element('ol');
  for(const tab of entries){
   const row=element('li');row.dataset.tabId=tab.tab_id;row.append(element('span',String(tab.position+1).padStart(2,'0'),'number'));const content=element('div');
   content.append(pageLink(tab));if(tab.pinned)content.append(element('span','Pinned','note'));if(mode==='domain')content.append(element('span','Window '+tab.window,'note'));
   content.append(element('div',tab.url,'url'));row.append(content);list.append(row);
  }
  section.append(list);fragment.append(section);
 }
 $('results').replaceChildren(fragment);
}
$('search').addEventListener('input',render);$('group').addEventListener('change',render);$('domain').addEventListener('change',render);
$('reset').addEventListener('click',()=>{$('search').value='';$('domain').value='';render();$('search').focus()});
render();
const target=new URLSearchParams(location.hash.slice(1)).get('tab');
const found=target&&document.querySelector('li[data-tab-id="'+CSS.escape(target)+'"]');
if(found){found.classList.add('target');found.scrollIntoView({block:'center'})}
</script>
</html>
""", SHARED_STYLES=SHARED_STYLES, SHARED_SCRIPT=SHARED_SCRIPT)

INDEX_TEMPLATE = fill(r"""<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Chrome tabs · Your archive</title>
<style>/* SHARED_STYLES */
.tools{grid-template-columns:1fr 170px 170px 200px}.archive{grid-template-columns:1fr auto;gap:24px;padding:24px 0}.archive-meta{text-align:right;font-size:14px}
.archive-meta a{color:var(--accent);font-size:12px;margin-left:12px}.archive-meta div{margin-top:8px}
.latest{display:inline-block;background:var(--accent);color:white;padding:12px 18px;border-radius:4px;margin-top:20px;text-decoration:none}
code{font-family:Menlo,monospace;font-size:.9em;background:#e7eadf;padding:3px 6px;border-radius:3px}.instructions{margin:28px 0;color:var(--muted)}
.views{display:flex;gap:8px;border-bottom:1px solid var(--line)}.views button{background:none;border:0;border-bottom:3px solid transparent;padding:10px 14px;color:var(--muted);font-weight:600;cursor:pointer}.views button[aria-pressed=true]{color:var(--ink);border-color:var(--accent)}
details{margin-top:4px;font-size:12px;color:var(--muted)}summary{cursor:pointer}details a{display:block;color:var(--accent);margin:4px 0 0 14px}.forget{background:none;border:0;padding:0;margin:8px 0 0 14px;font-size:12px;color:var(--muted);text-decoration:underline;cursor:pointer}#more{background:none;border:1px solid #a7afa1;border-radius:4px;padding:10px 16px;margin-bottom:30px;cursor:pointer}
@media(max-width:700px){.archive{grid-template-columns:1fr;gap:12px}.archive-meta{text-align:left}.archive-meta a{margin-left:0;margin-right:16px}}
</style>
<main>
<header><div class="eyebrow">Chrome / Personal archive</div><h1>Your tabs, kept.</h1>
<p>Every page you have saved, listed once. Open a page's history to find each snapshot and window it appeared in.</p>
<div class="stats" id="stats"></div>/* LATEST */</header>
<p class="instructions">Save another snapshot: <code>b_tabs_export</code><br>
Save and open this index: <code>b_tabs_export --open</code><br>
A session that matches the previous snapshot is skipped, so you can save as often as you like.</p>
<nav class="views" id="views" hidden><button id="show-pages" type="button">Pages</button><button id="show-snapshots" type="button">Snapshots</button></nav>
<div id="pages" hidden>
<div class="tools">
<label>Search your pages<input id="search" type="search" placeholder="Title, website, or URL" autocomplete="off"></label>
<label>Show<select id="status"><option value="">All pages</option><option value="open">Open in latest snapshot</option><option value="closed">Closed since</option></select></label>
<label>Sort by<select id="sort"><option value="last">Last seen</option><option value="first">Newest arrivals</option><option value="count">Most snapshots</option><option value="title">Title</option><option value="url">URL</option></select></label>
<label>Website<select id="domain"><option value="">All websites</option></select></label>
</div>
<div class="utility"><span id="count" role="status" aria-live="polite"></span><button id="reset" type="button">Clear filters</button></div>
<ol id="results"></ol><p id="empty" hidden>No pages match this search. Try another title or clear the filters.</p>
<button id="more" type="button" hidden></button>
</div>
<div id="snapshots">/* SNAPSHOTS */</div>
<footer>Stored locally. Each snapshot keeps its own HTML, Markdown, and JSON exports with its window layout. Pages are matched by exact URL. Local files and localhost pages are left out.<span id="forgotten"></span></footer>
</main>
<script id="data" type="application/json">/* LIBRARY_DATA */</script>
<script>
/* SHARED_SCRIPT */const library=JSON.parse($('data').textContent);
const snapshots=library.snapshots,pages=library.pages,PAGE_SIZE=300;
const day=date=>date.slice(0,10),minute=date=>date.slice(0,16).replace('T',' ');
pages.forEach((page,order)=>{page.order=order;page.last=snapshots[page.seen[0][0]].date;page.first=snapshots[page.seen.at(-1)[0]].date;page.open=page.seen[0][0]===0;page.address=page.url.replace(/^[a-z]+:\/\/(www\.)?/i,'');page.name=(page.title||page.address).replace(/^(\(\d+[^)]*\)\s*)?[^\p{L}\p{N}]*/u,'')});
const sorts={last:(a,b)=>a.order-b.order,first:(a,b)=>b.first.localeCompare(a.first)||a.order-b.order,count:(a,b)=>b.seen.length-a.seen.length||a.order-b.order,title:(a,b)=>a.name.localeCompare(b.name,undefined,{sensitivity:'base'})||a.order-b.order,url:(a,b)=>a.address.localeCompare(b.address)};
for(const [number,label] of [[pages.length,'Pages'],[pages.filter(p=>p.open).length,'Open in latest'],[snapshots.length,'Snapshots']]){const stat=element('div');stat.append(element('strong',number),element('span',label));$('stats').append(stat)}
const domains=new Map();for(const page of pages)domains.set(page.domain,(domains.get(page.domain)||0)+1);
for(const domain of [...domains.keys()].sort()){const option=element('option',domain+' · '+domains.get(domain));option.value=domain;$('domain').append(option)}
if(library.forgotten)$('forgotten').textContent=' Forgotten pages hidden: '+library.forgotten+'. They are listed in forgotten.json; bring one back with b_tabs_export --restore URL.';
let limit=PAGE_SIZE;
function history(page){
 const details=element('details');const count=page.seen.length;
 details.append(element('summary',(page.open?'Open in latest snapshot':'Last seen '+day(page.last))+(count>1?' · first seen '+day(page.first)+' · '+count+' snapshots':'')));
 for(const [number,places] of page.seen)for(const [windowNumber,tab] of places){const link=element('a',minute(snapshots[number].date)+' · Window '+windowNumber);link.href=snapshots[number].base+'/tabs.html#tab='+tab;details.append(link)}
 const forget=element('button','Copy forget command','forget');forget.type='button';
 forget.addEventListener('click',async()=>{const command="b_tabs_export --forget '"+page.url.replaceAll("'","'\\''")+"'";
  try{await navigator.clipboard.writeText(command);forget.textContent='Copied. Run it in a terminal to hide this page.'}catch{forget.replaceWith(element('code',command,'forget'))}});
 details.append(forget);return details;
}
function render(){
 const query=$('search').value.toLocaleLowerCase().trim(),domain=$('domain').value,status=$('status').value;
 const shown=pages.filter(p=>(!domain||p.domain===domain)&&(!status||p.open===(status==='open'))&&(!query||(p.title+' '+p.url).toLocaleLowerCase().includes(query))).sort(sorts[$('sort').value]);
 $('count').textContent=shown.length+' of '+pages.length+' pages';$('empty').hidden=shown.length>0;
 const fragment=document.createDocumentFragment();
 for(const page of shown.slice(0,limit)){
  const row=element('li');row.append(element('span',page.seen.length+'×','number'));const content=element('div');
  content.append(pageLink(page));if(page.open)content.append(element('span','Open','note'));
  content.append(element('div',page.url,'url'),history(page));row.append(content);fragment.append(row);
 }
 $('results').replaceChildren(fragment);
 $('more').hidden=shown.length<=limit;$('more').textContent='Show all '+shown.length+' pages';
}
function filter(){limit=PAGE_SIZE;render()}
function view(name){for(const other of ['pages','snapshots']){$(other).hidden=other!==name;$('show-'+other).setAttribute('aria-pressed',other===name)}}
$('search').addEventListener('input',filter);for(const id of ['status','sort','domain'])$(id).addEventListener('change',filter);
$('reset').addEventListener('click',()=>{$('search').value='';$('domain').value='';$('status').value='';filter();$('search').focus()});
$('more').addEventListener('click',()=>{limit=Infinity;render()});
$('show-pages').addEventListener('click',()=>view('pages'));$('show-snapshots').addEventListener('click',()=>view('snapshots'));
$('views').hidden=false;view(pages.length?'pages':'snapshots');render();
</script>
</html>
""", SHARED_STYLES=SHARED_STYLES, SHARED_SCRIPT=SHARED_SCRIPT)


if __name__ == '__main__':
    sys.exit(main())
