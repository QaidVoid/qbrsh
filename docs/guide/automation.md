# Automation (IPC)

qbrsh exposes a JSON-RPC control interface over a Unix domain socket, so any
process can drive the browser. This is the supported path for heavy or untrusted
automation, kept separate from in-process plugins.

## The socket

When qbrsh starts it binds a socket at:

```
$XDG_RUNTIME_DIR/qbrsh/ipc.sock
```

A stale socket left by a crashed instance is detected and replaced on the next
launch.

## Methods

Requests are newline-delimited JSON-RPC objects:

| Method | Params | Effect |
| --- | --- | --- |
| `run_command` | `{ "command": "<string>" }` | run a command, as if typed after `:` |
| `open_url` | `{ "url": "<string>" }` | open the URL in a new tab |

Each request gets a one-line response, `{"ok":true}` or `{"error":"..."}`.
Requests are delivered to the dispatch loop as messages and run on the main
loop; the socket thread never touches browser state directly.

## Examples

```sh
# Open a tab
printf '{"method":"open_url","params":{"url":"https://example.com"}}\n' \
  | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/qbrsh/ipc.sock

# Run any command
printf '{"method":"run_command","params":{"command":"tab-next"}}\n' \
  | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/qbrsh/ipc.sock
```

## URL forwarding

Launching `qbrsh <url>` while an instance is already running forwards the URL to
that instance over the same socket and exits, instead of starting a second
browser. With no running instance, it starts normally and creates the socket.
