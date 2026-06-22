//! The main window: a vertical tab list beside a view stack, with a status bar
//! and command line below the content.

use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Entry, Label, Orientation, Paned, PolicyType,
    ScrolledWindow,
};

/// Smallest the tab sidebar can be dragged to, in pixels. Low enough that the
/// collapsed icon-only rail is not clamped wider than intended.
const TAB_SIDEBAR_MIN: i32 = 28;

/// Handles to the window's widgets. Cloning is cheap (GObject reference counts).
#[derive(Clone)]
pub struct Ui {
    pub window: ApplicationWindow,
    pub layout_area: GtkBox,
    /// The draggable divider between the tab sidebar and the content; its
    /// position is the sidebar width.
    pub split: Paned,
    /// The vertical list of per-tab rows, rebuilt by `render_tabs`.
    pub tab_list: GtkBox,
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

        let tab_list = GtkBox::new(Orientation::Vertical, 0);
        tab_list.set_widget_name("qbrsh-tabs");
        let tab_scroll = ScrolledWindow::builder()
            .child(&tab_list)
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            // Do not let the rows' natural width drive the column; the divider
            // (or `tabs.width`) controls it.
            .propagate_natural_width(false)
            .build();
        tab_scroll.set_widget_name("qbrsh-tabs-scroll");
        tab_scroll.set_size_request(TAB_SIDEBAR_MIN, -1);

        // The right column: the content area over the status bar and command line.
        let right = GtkBox::new(Orientation::Vertical, 0);
        right.set_hexpand(true);
        right.set_vexpand(true);

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

        right.append(&layout_area);
        right.append(&completion);
        right.append(&statusbar);
        right.append(&commandline);

        // A draggable divider: the sidebar keeps its width when the window
        // resizes (the content takes the slack), and can shrink to the minimum.
        let split = Paned::new(Orientation::Horizontal);
        split.set_start_child(Some(&tab_scroll));
        split.set_end_child(Some(&right));
        split.set_resize_start_child(false);
        split.set_shrink_start_child(false);
        split.set_resize_end_child(true);
        split.set_shrink_end_child(true);
        window.set_child(Some(&split));

        Ui {
            window,
            layout_area,
            split,
            tab_list,
            statusbar,
            completion,
            commandline,
        }
    }
}
