use super::*;

use eframe::egui::text::LayoutJob;
use eframe::egui::{self, Align, Frame, Layout, Margin, ScrollArea, Stroke};

use crate::frontend::{
    actions::AppAction,
    state::{
        AppState, CommandActor, LogFilter, LogLevel, LogQuery, LogScope, OutputSource,
        OutputTarget, short_job,
    },
};

pub(crate) fn render_output_panel(state: &mut AppState, ui: &mut egui::Ui) {
    ui.set_width(ui.available_width());
    render_output_toolbar(state, ui);

    let target = state.ui.output.target.clone();
    let cleared = state
        .ui
        .output
        .cleared_before_by_target
        .get(&target)
        .copied()
        .unwrap_or(0);
    let search = state.ui.output.search.trim().to_string();
    let query = LogQuery::new(LogFilter::from_output_target(&target))
        .cleared_before(cleared)
        .search(&search);
    let latest_seq = state.session_log().latest_matching_seq(&query);
    let last_seen = state
        .ui
        .output
        .last_seen_by_target
        .get(&target)
        .copied()
        .unwrap_or(0);
    let unread = latest_seq.is_some_and(|seq| seq > last_seen);
    let auto_follow = state.ui.output.auto_follow;
    let wrap_mode = if state.ui.output.wrap_lines {
        egui::TextWrapMode::Wrap
    } else {
        egui::TextWrapMode::Extend
    };
    // Under a mixed view, a dim scope tag keeps interleaved sources legible; a
    // single-source or single-job view needs none.
    let show_prefix = matches!(
        target,
        OutputTarget::All | OutputTarget::Source(OutputSource::Jobs)
    );

    let log_width = ui.available_width();
    let log_content_width = (log_width - ASSISTANT_SCROLLBAR_RESERVE).max(48.0);

    let scroll = ScrollArea::vertical()
        .max_width(log_width)
        .auto_shrink([false, false])
        .content_margin(Margin::ZERO)
        .stick_to_bottom(auto_follow)
        .show(ui, |ui| {
            ui.set_width(log_content_width);
            let pal = crate::frontend::theme::palette(ui);
            if state.session_log().any(&query) {
                let transcript = build_output_transcript(state, &query, &pal, show_prefix);
                ui.add(
                    egui::Label::new(transcript)
                        .selectable(true)
                        .wrap_mode(wrap_mode),
                );
            } else {
                ui.add_space(6.0);
                ui.label(
                    console_text(output_empty_text(&target, &search)).color(pal.text_tertiary),
                );
            }
        });

    let at_bottom =
        scroll.state.offset.y >= scroll.content_size.y - scroll.inner_rect.height() - 2.0;
    state.ui.output.auto_follow = at_bottom;
    if at_bottom && let Some(seq) = latest_seq {
        state
            .ui
            .output
            .last_seen_by_target
            .insert(target.clone(), seq);
    }
    if unread && !at_bottom {
        ui.vertical_centered(|ui| {
            if ui.small_button("New output ↓").clicked() {
                state.ui.output.auto_follow = true;
                if let Some(seq) = latest_seq {
                    state
                        .ui
                        .output
                        .last_seen_by_target
                        .insert(target.clone(), seq);
                }
            }
        });
    }
}

/// The Output toolbar: the source selector, an exact-job selector when a job view
/// is active, then (right-aligned) search, wrap, and clear-view.
fn render_output_toolbar(state: &mut AppState, ui: &mut egui::Ui) {
    let current = state.ui.output.target.clone();
    let mut next_target: Option<OutputTarget> = None;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("output_source_selector")
            .selected_text(source_label(&current))
            .show_ui(ui, |ui| {
                let mut option = |ui: &mut egui::Ui, target: OutputTarget, label: &str| {
                    if ui
                        .selectable_label(source_matches(&current, &target), label)
                        .clicked()
                    {
                        next_target = Some(target);
                    }
                };
                option(ui, OutputTarget::All, "All Output");
                for source in OutputSource::all() {
                    option(ui, OutputTarget::Source(source), source.label());
                }
            });

        if matches!(
            current,
            OutputTarget::Source(OutputSource::Jobs) | OutputTarget::Job(_)
        ) {
            let choices = output_job_choices(state);
            let selected = match &current {
                OutputTarget::Job(job_id) => choices
                    .iter()
                    .find(|(id, _)| id == job_id)
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| format!("Job {}", short_job(*job_id))),
                _ => "All jobs".to_string(),
            };
            egui::ComboBox::from_id_salt("output_job_selector")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(
                            matches!(current, OutputTarget::Source(OutputSource::Jobs)),
                            "All jobs",
                        )
                        .clicked()
                    {
                        next_target = Some(OutputTarget::Source(OutputSource::Jobs));
                    }
                    for (job_id, label) in &choices {
                        if ui
                            .selectable_label(current == OutputTarget::Job(*job_id), label)
                            .clicked()
                        {
                            next_target = Some(OutputTarget::Job(*job_id));
                        }
                    }
                });
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button("Clear")
                .on_hover_text("Hide the current output (does not delete it)")
                .clicked()
            {
                let next = state.clear_log_fold_boundary();
                state
                    .ui
                    .output
                    .cleared_before_by_target
                    .insert(current.clone(), next);
                state
                    .ui
                    .output
                    .last_seen_by_target
                    .insert(current.clone(), next);
                state.ui.output.auto_follow = true;
            }
            let wrap = state.ui.output.wrap_lines;
            if ui
                .selectable_label(wrap, "Wrap")
                .on_hover_text("Wrap long lines")
                .clicked()
            {
                state.ui.output.wrap_lines = !wrap;
            }
            ui.add(
                egui::TextEdit::singleline(&mut state.ui.output.search)
                    .hint_text("Search")
                    .desired_width(140.0),
            );
        });
    });
    if let Some(target) = next_target {
        state.ui.output.target = target;
        state.ui.output.auto_follow = true;
    }
}

/// The exact-job choices for the selector: live/recent jobs from the execution
/// projection unioned with jobs that still own retained log entries, so a choice
/// never vanishes merely because its oldest text was evicted.
fn output_job_choices(state: &AppState) -> Vec<(crate::job::JobId, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut choices = Vec::new();
    for snapshot in crate::frontend::jobs::list_controlled_jobs(state) {
        if let Some(job_id) = snapshot.job_id
            && seen.insert(job_id)
        {
            choices.push((
                job_id,
                format!(
                    "{} · {} ({})",
                    snapshot.kind.label(),
                    snapshot.label,
                    short_job(job_id)
                ),
            ));
        }
    }
    for job_id in state.session_log().logged_jobs() {
        if seen.insert(job_id) {
            choices.push((job_id, format!("Job {}", short_job(job_id))));
        }
    }
    choices
}

fn source_label(target: &OutputTarget) -> String {
    match target {
        OutputTarget::All => "All Output".to_string(),
        OutputTarget::Source(source) => source.label().to_string(),
        // A job target keeps the source selector reading "Jobs"; the job selector
        // beside it names the exact job.
        OutputTarget::Job(_) => OutputSource::Jobs.label().to_string(),
    }
}

/// Whether the source selector should mark `option` as the current source (a Job
/// target counts as the Jobs source).
fn source_matches(current: &OutputTarget, option: &OutputTarget) -> bool {
    match (current, option) {
        (OutputTarget::Job(_), OutputTarget::Source(OutputSource::Jobs)) => true,
        _ => current == option,
    }
}

fn output_empty_text(target: &OutputTarget, search: &str) -> String {
    if !search.is_empty() {
        return format!("No output matches \"{search}\".");
    }
    match target {
        OutputTarget::Job(_) => "No output captured for this job yet.".to_string(),
        OutputTarget::Source(source) => format!("No {} output yet.", source.label()),
        OutputTarget::All => "No output yet.".to_string(),
    }
}

/// Build the Output body as one selectable, copyable [`LayoutJob`], coloring Warn
/// and Error even without a level filter and, in a mixed view, tagging each line
/// with its scope.
fn build_output_transcript(
    state: &AppState,
    query: &LogQuery,
    pal: &crate::frontend::theme::Palette,
    show_prefix: bool,
) -> LayoutJob {
    let font = console_font_id();
    let mut job = LayoutJob::default();
    for entry in state.session_log().query(query) {
        if show_prefix {
            push_run(
                &mut job,
                &scope_prefix(&entry.scope),
                &font,
                pal.text_tertiary,
            );
        }
        let color = level_color(entry.level, pal);
        push_run(&mut job, &entry.text, &font, color);
        if entry.repeat_count() > 1 {
            push_run(
                &mut job,
                &format!("  ×{}", entry.repeat_count()),
                &font,
                pal.text_tertiary,
            );
        }
        push_run(&mut job, "\n", &font, color);
    }
    job
}

fn level_color(level: LogLevel, pal: &crate::frontend::theme::Palette) -> egui::Color32 {
    match level {
        LogLevel::Error => pal.status_red,
        LogLevel::Warn => pal.status_amber,
        LogLevel::Trace => pal.text_tertiary,
        LogLevel::Info => pal.text_primary,
    }
}

fn scope_prefix(scope: &LogScope) -> String {
    match scope {
        LogScope::System { subsystem } => format!("{}: ", subsystem.label().to_lowercase()),
        LogScope::Agent { .. } => "agent: ".to_string(),
        LogScope::RemoteControl { .. } => "remote: ".to_string(),
        LogScope::Job { job_id } => format!("job {}: ", short_job(*job_id)),
        LogScope::Command { .. } => String::new(),
    }
}

pub(crate) fn render_console_panel(
    state: &mut AppState,
    ui: &mut egui::Ui,
    actions: &mut Vec<AppAction>,
) {
    const PROMPT_ROW_HEIGHT: f32 = 34.0;
    const TOOLBAR_HEIGHT: f32 = 22.0;
    const INPUT_OUTER_HEIGHT: f32 = 28.0;
    const INPUT_X_MARGIN: f32 = 8.0;
    const DIVIDER_HEIGHT: f32 = 1.0;
    const BOTTOM_PADDING: f32 = 4.0;

    ui.set_width(ui.available_width());

    let query = LogQuery::new(LogFilter::Command).cleared_before(state.ui.console.cleared_before);
    let latest_seq = state.session_log().latest_matching_seq(&query);
    let unread = latest_seq.is_some_and(|seq| seq > state.ui.console.last_seen);

    render_console_toolbar(state, ui);

    // Keep chronological output in top-down visual order while reserving fixed
    // space for the prompt row so the panel cannot grow frame-over-frame.
    let log_height = (ui.available_height()
        - PROMPT_ROW_HEIGHT
        - TOOLBAR_HEIGHT
        - DIVIDER_HEIGHT
        - BOTTOM_PADDING)
        .max(0.0);
    let log_width = ui.available_width();
    let log_content_width = (log_width - ASSISTANT_SCROLLBAR_RESERVE).max(48.0);
    let wrap_mode = if state.ui.console.wrap_lines {
        egui::TextWrapMode::Wrap
    } else {
        egui::TextWrapMode::Extend
    };
    let auto_follow = state.ui.console.auto_follow;

    let scroll = ui
        .allocate_ui_with_layout(
            egui::vec2(log_width, log_height),
            Layout::top_down(Align::Min),
            |ui| {
                ScrollArea::vertical()
                    .max_width(log_width)
                    .auto_shrink([false, false])
                    .content_margin(Margin::ZERO)
                    .stick_to_bottom(auto_follow)
                    .show(ui, |ui| {
                        ui.set_width(log_content_width);
                        let pal = crate::frontend::theme::palette(ui);
                        if state.session_log().any(&query) {
                            let transcript = build_transcript(state, &query, &pal);
                            ui.add(
                                egui::Label::new(transcript)
                                    .selectable(true)
                                    .wrap_mode(wrap_mode),
                            );
                        } else {
                            ui.add_space(6.0);
                            ui.label(
                                console_text("Run a .sls command below — try `help`. Enter runs.")
                                    .color(pal.text_tertiary),
                            );
                        }
                    })
            },
        )
        .inner;

    // Auto-follow tracks whether the view is parked at the bottom, so incoming
    // output only steals the scroll position when the user was already there.
    let at_bottom =
        scroll.state.offset.y >= scroll.content_size.y - scroll.inner_rect.height() - 2.0;
    state.ui.console.auto_follow = at_bottom;
    if at_bottom && let Some(seq) = latest_seq {
        state.ui.console.last_seen = seq;
    }

    if unread && !at_bottom {
        ui.vertical_centered(|ui| {
            if ui.small_button("New output ↓").clicked() {
                state.ui.console.auto_follow = true;
                if let Some(seq) = latest_seq {
                    state.ui.console.last_seen = seq;
                }
            }
        });
    }

    weak_panel_hairline(ui, 14);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), PROMPT_ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            let pal = crate::frontend::theme::palette(ui);
            let input_radius =
                crate::frontend::theme::radius::concentric(crate::frontend::theme::radius::CARD, 2);
            ui.spacing_mut().item_spacing.x = 8.0;

            ui.label(console_text("sls>"));

            let button_width = 46.0;
            let text_edit_width = (ui.available_width()
                - button_width
                - ui.spacing().item_spacing.x
                - INPUT_X_MARGIN * 2.0)
                .max(96.0);

            let response = Frame::default()
                .fill(pal.input_fill)
                .stroke(Stroke::new(1.0_f32, pal.hairline))
                .corner_radius(egui::CornerRadius::same(input_radius))
                .inner_margin(Margin::symmetric(INPUT_X_MARGIN as i8, 3))
                .show(ui, |ui| {
                    ui.add_sized(
                        [text_edit_width, INPUT_OUTER_HEIGHT - 8.0],
                        egui::TextEdit::singleline(&mut state.ui.console.input)
                            .desired_width(f32::INFINITY)
                            .frame(Frame::NONE)
                            .margin(Margin::ZERO)
                            .hint_text("view background white"),
                    )
                })
                .inner;

            let mut run = false;
            if ui.button("Run").clicked() {
                run = true;
            }

            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                run = true;
            }

            if run {
                let command = state.ui.console.input.trim().to_string();
                if !command.is_empty() {
                    actions.push(AppAction::RunConsoleCommand(command));
                    state.ui.console.input.clear();
                }
            }
        },
    );
    ui.add_space(BOTTOM_PADDING);
}

/// The Console's slim control row: wrap toggle and clear-view, right-aligned.
fn render_console_toolbar(state: &mut AppState, ui: &mut egui::Ui) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 20.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            if ui
                .button("Clear")
                .on_hover_text("Hide the current transcript (does not delete it)")
                .clicked()
            {
                let next = state.clear_log_fold_boundary();
                state.ui.console.cleared_before = next;
                state.ui.console.last_seen = next;
                state.ui.console.auto_follow = true;
            }
            let wrap = state.ui.console.wrap_lines;
            if ui
                .selectable_label(wrap, "Wrap")
                .on_hover_text("Wrap long lines")
                .clicked()
            {
                state.ui.console.wrap_lines = !wrap;
            }
        },
    );
}

/// Build the Console transcript as one selectable, copyable [`LayoutJob`]:
/// each command's prompt line is prefixed by its actor, and result/error lines
/// carry level color and an `×N` repetition badge.
fn build_transcript(
    state: &AppState,
    query: &LogQuery,
    pal: &crate::frontend::theme::Palette,
) -> LayoutJob {
    let font = console_font_id();
    let mut job = LayoutJob::default();
    let mut current: Option<u64> = None;
    for entry in state.session_log().query(query) {
        let LogScope::Command { command_id, actor } = entry.scope else {
            continue;
        };
        if current != Some(command_id) {
            current = Some(command_id);
            let prefix = match actor {
                CommandActor::User => "sls> ",
                CommandActor::Agent => "agent> ",
            };
            push_run(&mut job, prefix, &font, pal.text_tertiary);
            push_run(
                &mut job,
                &format!("{}\n", entry.text),
                &font,
                pal.text_primary,
            );
            continue;
        }
        let color = match entry.level {
            LogLevel::Error => pal.status_red,
            LogLevel::Warn => pal.status_amber,
            LogLevel::Trace => pal.text_tertiary,
            LogLevel::Info => pal.text_primary,
        };
        push_run(&mut job, &entry.text, &font, color);
        if entry.repeat_count() > 1 {
            push_run(
                &mut job,
                &format!("  ×{}", entry.repeat_count()),
                &font,
                pal.text_tertiary,
            );
        }
        push_run(&mut job, "\n", &font, color);
    }
    job
}

fn push_run(job: &mut LayoutJob, text: &str, font: &egui::FontId, color: egui::Color32) {
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
}
