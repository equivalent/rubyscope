//! Closing the window. The close is held back while the work that must
//! finish runs, and a dialog shows what runs when it takes a moment.
//! Then the app quits without freeing its memory: the system does that.

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};

use crate::theme::Theme;

/// The dialog shows only when closing takes longer than this, so a quick
/// close does not flash it.
const SHOW_AFTER: Duration = Duration::from_millis(150);

/// After this the app quits, even when a step has not finished.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    SaveTabs,
    StopWatcher,
    SaveIndex,
    Pull,
    SwitchBranch,
}

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Step::SaveTabs => "Saving the open tabs",
            Step::StopWatcher => "Stopping the file watcher",
            Step::SaveIndex => "Saving the index",
            Step::Pull => "Waiting for git pull",
            Step::SwitchBranch => "Waiting for the branch switch",
        }
    }
}

pub struct Closing {
    pub started: Instant,
    pub steps: Vec<Step>,
    /// How many steps are done.
    pub done: usize,
}

impl Closing {
    pub fn new(steps: Vec<Step>) -> Closing {
        Closing { started: Instant::now(), steps, done: 0 }
    }

    pub fn current(&self) -> Option<Step> {
        self.steps.get(self.done).copied()
    }

    pub fn given_up(&self) -> bool {
        self.started.elapsed() >= GIVE_UP_AFTER
    }

    /// The dialog: the step that runs and a bar over all steps.
    pub fn ui(&self, ctx: &egui::Context, theme: &Theme) {
        if self.started.elapsed() < SHOW_AFTER {
            return;
        }
        let Some(step) = self.current() else { return };
        let progress = self.done as f32 / self.steps.len().max(1) as f32;
        egui::Modal::new(egui::Id::unique("closing"))
            .frame(egui::Frame::popup(&ctx.global_style()).fill(theme.bg_light).inner_margin(14.0))
            .show(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new("Closing rubyscope").strong().color(theme.fg_bright));
                ui.add_space(6.0);
                ui.label(RichText::new(format!("{}…", step.label())).color(theme.fg));
                ui.add_space(6.0);
                ui.add(egui::ProgressBar::new(progress).desired_width(320.0).fill(theme.accent));
                ui.add_space(6.0);
                ui.label(RichText::new("Close it again to quit now.").small().color(theme.fg_dim));
            });
    }
}
