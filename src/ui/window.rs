//! The main window: a view stack with a tab bar, status bar, and command line.

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Entry, Label, Orientation};

/// Handles to the window's widgets. Cloning is cheap (GObject reference counts).
#[derive(Clone)]
pub struct Ui {
    pub window: ApplicationWindow,
    pub layout_area: GtkBox,
    pub tabbar: Label,
    pub statusbar: Label,
    pub completion: GtkBox,
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
        tabbar.set_widget_name("qbrsh-tabbar");

        // The layout area holds a single child: the focused pane's wrapper, or a
        // GtkPaned tree when split. It is repopulated by `render_layout`.
        let layout_area = GtkBox::new(Orientation::Vertical, 0);
        layout_area.set_vexpand(true);
        layout_area.set_hexpand(true);
        layout_area.set_widget_name("qbrsh-layout");

        let statusbar = Label::new(None);
        statusbar.set_xalign(0.0);
        statusbar.set_widget_name("qbrsh-status");

        let completion = GtkBox::new(Orientation::Vertical, 0);
        completion.set_visible(false);
        completion.set_widget_name("qbrsh-completion");

        let commandline = Entry::new();
        commandline.set_visible(false);
        commandline.set_widget_name("qbrsh-cmd");

        vbox.append(&tabbar);
        vbox.append(&layout_area);
        vbox.append(&completion);
        vbox.append(&statusbar);
        vbox.append(&commandline);
        window.set_child(Some(&vbox));

        Ui {
            window,
            layout_area,
            tabbar,
            statusbar,
            completion,
            commandline,
        }
    }
}
