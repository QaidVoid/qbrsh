# Plugin API

Plugins are Rune scripts in `~/.local/share/qbrsh/plugins/*.rn`. The browser
exposes a single module, `qbrsh`. See [Plugins](/guide/plugins) for an overview.

## Functions

| Function | Description |
| --- | --- |
| `qbrsh::command(s)` | run a command string, as if typed after `:` |
| `qbrsh::open(url)` | open a URL in the current tab |
| `qbrsh::message(s)` | show a status-bar message |
| `qbrsh::eval_js(s).await` | evaluate JavaScript in the active tab and return its result as a string |
| `qbrsh::on(event, handler)` | register a cold-event hook by handler name |

`eval_js` is awaitable: it suspends the plugin, asks the browser to evaluate the
script in the active tab, and resumes with the result. The fire-and-forget
functions enqueue an action and return immediately.

## Events

`on(event, handler)` registers a top-level function (by name) to run when an
event fires. Available cold events:

| Event | Argument | Fires when |
| --- | --- | --- |
| `page_load` | the page URL | a page finishes loading |
| `tab_open` | the URL | a tab is opened |
| `command` | the command text | a command is run from the command line |

Hot paths (per network request, per keystroke, per frame) are intentionally not
exposed; use native ad blocking or keybindings for those.

## Lifecycle

- `main()` runs once at load. Use it to register hooks. It should be synchronous
  and must not await.
- Hooks may be `async` and may await `eval_js`.
- Each invocation runs under an instruction budget; a runaway hook is aborted.
- Reload all plugins with `:plugin-reload`.

## Example

```rust
pub fn main() {
    qbrsh::on("page_load", "greet");
    qbrsh::on("command", "log_command");
}

pub async fn greet(url) {
    if url.starts_with("https://github.com") {
        let title = qbrsh::eval_js("document.title").await;
        qbrsh::message(`on GitHub: ${title}`);
    }
}

pub fn log_command(cmd) {
    qbrsh::message(`ran: ${cmd}`);
}
```
