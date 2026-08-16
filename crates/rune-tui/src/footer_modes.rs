use ratatui::text::Span;

use crate::app::App;
use crate::footer_hints::hint_entry_spans;
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::keymap::GlobalCommand;

pub(crate) fn hint_row<const N: usize>(
    app: &App,
    entries: [(&str, &'static str); N],
) -> Vec<Span<'static>> {
    entries
        .into_iter()
        .enumerate()
        .flat_map(|(i, (key, help))| hint_entry_spans(&app.theme, i, key.to_string(), help, true))
        .collect()
}

pub(crate) fn disk_changed_spans(app: &App) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        "\u{21c4} disk changed",
        app.theme.chrome.footer_hint,
    )];
    if let Some((label, help)) = crate::global::hint_for(GlobalCommand::Merge) {
        spans.extend(hint_entry_spans(&app.theme, 1, label, help, true));
    }
    spans
}

pub(crate) fn merge_hint_spans(app: &App, unresolved: usize) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        "\u{2699} merge \u{2014} ",
        app.theme.chrome.footer_hint,
    )];
    spans.extend(hint_row(
        app,
        [
            ("O", "ours"),
            ("T", "theirs"),
            ("B", "both"),
            ("[", "prev"),
            ("]", "next"),
        ],
    ));
    spans.push(Span::styled(
        format!("  · {unresolved} left"),
        app.theme.chrome.footer_hint,
    ));
    spans
}

/// A Guard's prompt text plus its answer hints, built from the SAME
/// `guard::GuardOption` consts `handle_guard_key` dispatches on, so this
/// render can never drift from what those keys actually do.
pub(crate) fn guard_spans(app: &App, prompt: &GuardPrompt) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    // A rename collision names its target: "replace <what>?" is a question
    // the user can answer; a bare "replace" is not. `DirtyClose`/
    // `DirtyQuit` both name WHICH document is waiting, so the prompt matches
    // something the tab bar already shows. Deliberately NOT `title::name_for`:
    // that one answers "what should the title FIELD hold", which for a
    // pathless draft is the editable `.md` stub — a prompt reading "unsaved
    // changes in .md" names nothing at all.
    let options: &[guard::GuardOption] = match &prompt.kind {
        GuardKind::DirtyClose | GuardKind::DirtyQuit => {
            let name = app
                .doc(prompt.doc)
                .map(|doc| doc.file_name().to_string())
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("unsaved changes in {name} \u{2014} "),
                app.theme.chrome.footer_hint,
            ));
            guard::DIRTY_CLOSE_OPTIONS
        }
        GuardKind::RenameCollision { target } => {
            spans.push(Span::styled(
                format!("{target} already exists  "),
                app.theme.chrome.footer_hint,
            ));
            // Without a durable store there is nowhere to preserve
            // the replaced file's bytes, so the option is not offered at
            // all — an offer the app would then refuse is worse than none.
            if crate::rename::replace_allowed(app) {
                guard::RENAME_COLLISION_OPTIONS
            } else {
                &[]
            }
        }
        GuardKind::DiskConflict => {
            let name = app
                .doc(prompt.doc)
                .map(|doc| doc.file_name().to_string())
                .unwrap_or_default();
            spans.push(Span::styled(
                format!("{name} changed on disk \u{2014} "),
                app.theme.chrome.footer_hint,
            ));
            guard::DISK_CONFLICT_OPTIONS
        }
        GuardKind::Trash { path } => {
            let name = crate::trash::display_name(path);
            spans.push(Span::styled(
                format!("Trash {name}? "),
                app.theme.chrome.footer_hint,
            ));
            guard::TRASH_OPTIONS
        }
    };

    for (i, opt) in options.iter().chain([&guard::GUARD_CANCEL]).enumerate() {
        spans.extend(hint_entry_spans(
            &app.theme,
            i,
            opt.key.label(),
            opt.help,
            true,
        ));
    }
    spans
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::app::App;
    use crate::footer::footer_text;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn merge_hint_names_one_key_per_action() {
        let mut app = app_with("hello");
        app.merge = crate::merge::MergeState::Active {
            doc: app.active,
            session: crate::merge::MergeSession {
                conflicts: Vec::new(),
                cur: 0,
                saved_display_name: None,
                theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
            },
        };

        assert_eq!(
            footer_text(&app),
            "\u{2699} merge \u{2014} O ours  T theirs  B both  [ prev  ] next  · 0 left"
        );
    }

    #[test]
    fn guard_mode_offers_every_answer_as_a_plain_key_hint() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.guard = Some(crate::guard::GuardPrompt {
            doc,
            kind: crate::guard::GuardKind::DirtyClose,
        });

        let text = footer_text(&app);
        for opt in crate::guard::DIRTY_CLOSE_OPTIONS
            .iter()
            .chain([&crate::guard::GUARD_CANCEL])
        {
            let hint = format!("{} {}", opt.key.label(), opt.help);
            assert!(
                text.contains(&hint),
                "expected {hint:?} in the Guard footer text {text:?}"
            );
        }
        assert!(
            text.ends_with("S save  D discard  Esc cancel"),
            "key hints carry no brackets: {text:?}"
        );
    }
}
