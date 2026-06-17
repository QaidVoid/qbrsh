//! GDK key-event translation and the Normal-mode key controller.
//!
//! This is the only place GDK key types are handled. Raw key events are
//! translated into the toolkit-independent [`Key`] and enqueued as messages; the
//! binding trie and `update` interpret them. A small read-only [`ModeMirror`]
//! lets the controller make its synchronous propagation decision without
//! touching the state owned by the dispatch loop.

use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{EventControllerKey, PropagationPhase};

use crate::core::command::Command;
use crate::core::key::Key;
use crate::core::msg::Msg;
use crate::core::runtime::Mailbox;
use crate::core::state::Mode;
use crate::ui::window::Ui;

/// Read-only view of the input state for the controller's synchronous
/// propagation decision. The dispatch loop writes it after each message.
#[derive(Clone, Copy)]
pub struct UiView {
    pub mode: Mode,
    /// Whether a completion candidate is currently highlighted.
    pub completion_active: bool,
}

/// Shared handle to the [`UiView`] read by the key controller.
pub type ModeMirror = Rc<Cell<UiView>>;

/// Install the key controller and command-line signal handlers.
///
/// Returns the [`ModeMirror`] that the dispatch loop must keep in sync with the
/// current mode and completion selection.
pub fn install(ui: &Ui, mailbox: &Mailbox) -> ModeMirror {
    let mirror: ModeMirror = Rc::new(Cell::new(UiView {
        mode: Mode::Normal,
        completion_active: false,
    }));

    let controller = EventControllerKey::new();
    controller.set_propagation_phase(PropagationPhase::Capture);
    let mb = mailbox.clone();
    let view = mirror.clone();
    controller.connect_key_pressed(move |_, keyval, _, mods| {
        let Some(key) = to_key(keyval, mods) else {
            return glib::Propagation::Proceed;
        };
        // Escape leaves the current mode regardless of which mode is active.
        if key.sym == "Escape" && !key.ctrl && !key.alt {
            mb.send(Msg::Command(Command::ModeLeave));
            return glib::Propagation::Stop;
        }
        let snapshot = view.get();
        match snapshot.mode {
            // The command entry handles typing and Enter; Tab cycles completion,
            // and Space applies the highlighted candidate (else a literal space).
            Mode::Command => {
                if key.sym == "Tab" && !key.ctrl && !key.alt {
                    mb.send(if key.shift {
                        Msg::CompletionPrev
                    } else {
                        Msg::CompletionNext
                    });
                    return glib::Propagation::Stop;
                }
                if key.sym == "space" && !key.ctrl && !key.alt && snapshot.completion_active {
                    mb.send(Msg::CompletionApply);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            // Insert mode forwards keys to the page.
            Mode::Insert => glib::Propagation::Proceed,
            // Normal, Hint, Prompt, and Permissions modes route every key through
            // the core.
            Mode::Normal | Mode::Hint | Mode::Prompt | Mode::Permissions => {
                mb.send(Msg::Key(key));
                glib::Propagation::Stop
            }
        }
    });
    ui.window.add_controller(controller);

    let mb = mailbox.clone();
    ui.commandline
        .connect_activate(move |_| mb.send(Msg::Command(Command::Accept)));

    let mb = mailbox.clone();
    ui.commandline
        .connect_changed(move |e| mb.send(Msg::CommandLineChanged(e.text().to_string())));

    mirror
}

/// Translate a GDK key press into the toolkit-independent [`Key`].
/// Returns `None` for modifier-only or non-textual keys.
fn to_key(keyval: gdk4::Key, mods: gdk4::ModifierType) -> Option<Key> {
    use gdk4::ModifierType;
    let ctrl = mods.contains(ModifierType::CONTROL_MASK);
    let alt = mods.contains(ModifierType::ALT_MASK);

    if let Some(sym) = named_sym(keyval) {
        let shift = mods.contains(ModifierType::SHIFT_MASK);
        return Some(Key {
            sym,
            ctrl,
            alt,
            shift,
        });
    }
    let c = keyval.to_unicode()?;
    if c.is_control() {
        return None;
    }
    // For printable keys the shifted form is already encoded in the character.
    Some(Key {
        sym: c.to_string(),
        ctrl,
        alt,
        shift: false,
    })
}

/// Map a named (non-printable) GDK key to its canonical symbol.
fn named_sym(keyval: gdk4::Key) -> Option<String> {
    let name = match keyval {
        gdk4::Key::Escape => "Escape",
        gdk4::Key::Return | gdk4::Key::KP_Enter => "Return",
        gdk4::Key::Tab | gdk4::Key::ISO_Left_Tab => "Tab",
        gdk4::Key::space => "space",
        gdk4::Key::BackSpace => "BackSpace",
        gdk4::Key::Delete => "Delete",
        gdk4::Key::Insert => "Insert",
        gdk4::Key::Up => "Up",
        gdk4::Key::Down => "Down",
        gdk4::Key::Left => "Left",
        gdk4::Key::Right => "Right",
        gdk4::Key::Page_Up => "PgUp",
        gdk4::Key::Page_Down => "PgDown",
        gdk4::Key::Home => "Home",
        gdk4::Key::End => "End",
        gdk4::Key::F1 => "F1",
        gdk4::Key::F2 => "F2",
        gdk4::Key::F3 => "F3",
        gdk4::Key::F4 => "F4",
        gdk4::Key::F5 => "F5",
        gdk4::Key::F6 => "F6",
        gdk4::Key::F7 => "F7",
        gdk4::Key::F8 => "F8",
        gdk4::Key::F9 => "F9",
        gdk4::Key::F10 => "F10",
        gdk4::Key::F11 => "F11",
        gdk4::Key::F12 => "F12",
        _ => return None,
    };
    Some(name.to_string())
}
