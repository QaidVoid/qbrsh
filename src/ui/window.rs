//! The main window: a view stack with a tab bar, status bar, and command line.

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Entry, Label, Orientation, Stack};

/// Handles to the window's widgets. Cloning is cheap (GObject reference counts).
#[derive(Clone)]
pub struct Ui {
    pub window: ApplicationWindow,
    pub stack: Stack,
    pub tabbar: Label,
    pub statusbar: Label,
    pub commandline: Entry,
}

impl Ui {
    /// Build the window and its layout for `app`.
    pub fn build(app: &Application) -> Ui {
        let window = ApplicationWindow::builder()
            .application(app)
            .default_width(1200)
            .default_height(800)
            .title("qbrsh")
            .build();

        let vbox = GtkBox::new(Orientation::Vertical, 0);

        let tabbar = Label::new(None);
        tabbar.set_xalign(0.0);

        let stack = Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);

        let statusbar = Label::new(None);
        statusbar.set_xalign(0.0);

        let commandline = Entry::new();
        commandline.set_visible(false);

        vbox.append(&tabbar);
        vbox.append(&stack);
        vbox.append(&statusbar);
        vbox.append(&commandline);
        window.set_child(Some(&vbox));

        Ui {
            window,
            stack,
            tabbar,
            statusbar,
            commandline,
        }
    }
}
