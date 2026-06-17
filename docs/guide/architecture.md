# Architecture

qbrsh is built on a hand-rolled Elm-style architecture, often called TEA (The
Elm Architecture). The goal is a browser that stays responsive and is easy to
reason about, without the shared mutable state and re-entrancy that tend to
accumulate in GTK applications.

## The loop

```
sources -> Msg queue -> update(&mut State) -> [Effect] -> runner -> results back as Msg
```

- **State** is one owned value. Every subsystem (modes, tabs, input, completion,
  config, hints) is a plain field on it. There is no `Rc<RefCell>` sharing of
  state across subsystems.
- **Msg** is the single type every source of change produces: key presses,
  WebKit signals, IPC requests, worker results, and so on.
- **update** is the only place state mutates. It is synchronous, does no I/O,
  and returns a list of effects to perform. Because it is pure, it is fully
  unit-tested without GTK.
- **Effect** values are carried out by an effect runner after update returns.
  Effects that produce a value (such as evaluating JavaScript) deliver the
  result back as a new message.

A single consumer drains the message queue on glib's main context. Nothing
mutates state inside a signal callback, so there is no re-entrancy and no need
for defensive borrow guards. There are no polling timers.

## Where the code lives

| Path | Responsibility |
| --- | --- |
| `src/core/` | the engine-agnostic TEA core: state, msg, effect, update, command, key handling, completion. Unit-tested without GTK. |
| `src/engine/` | the `EngineView` trait and the WebKitGTK backend, the only place WebKit types appear. |
| `src/input/`, `src/ui/`, `src/app.rs` | GDK key translation, the window, and the effect runner that drives GTK and WebKit. |
| `src/history.rs` | browsing history on a dedicated SQLite writer thread. |
| `src/plugin.rs` | the Rune plugin runtime. |
| `src/ipc.rs` | the JSON-RPC control socket. |

## Threading

Asynchronous work runs on glib's main context; there is no second runtime.
Blocking work is offloaded to worker threads that report results back as
messages. History writes happen on a single dedicated writer thread, and ad
filter compilation runs off the main thread. The mailbox that carries messages
is thread-safe, so any worker can hand results back to the main loop.

## Hot paths stay native

Decisions that must be fast and synchronous, such as ad blocking in the
navigation handler and permission requests, are made directly in the engine
rather than routed through the message loop. Plugins only attach to cold events
(page load, tab open, command) and never to per-request or per-keystroke paths.
