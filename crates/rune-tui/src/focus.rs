//! `FocusTarget` — the `when`-clause-facing view of "what has focus" (plan
//! WP6.S3), deliberately a SEPARATE type from `pane::Pane`. `Pane` stays the
//! chrome-region discriminant `app::handle_key`'s stage-3 dispatch already
//! keys off of; `FocusTarget` is the vocabulary `when.rs` clauses are
//! written against, and it needs variants `Pane` doesn't have (the
//! search/replace fields land in WP8).

use ratatui::layout::Rect;

use crate::app::App;
use crate::document::ReadOnly;
use crate::pane::Pane;
use crate::runtime::Effects;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    Explorer,
    Tabs,
    Editor,
    /// The editable title field (`title.rs`, `Pane::Title`) — reachable
    /// today via `^r` or the Up-at-editor-top gesture.
    Title,
    /// Not yet reachable — WP8 adds the search UI this pairs with. Included
    /// now so a `when` clause authored against it (e.g. a future search-bar
    /// binding table) parses and validates today, ahead of the field that
    /// will actually produce it.
    SearchField,
    /// Not yet reachable — see `SearchField`'s doc; WP8's replace field.
    ReplaceField,
}

/// Derives today's `FocusTarget` from the chrome-level `Pane` — the only
/// input that exists before WP8's search state lands. Once a search bar
/// exists to focus, its own state (not `Pane`, which never grows a
/// `Pane::Search` variant per the plan's decision 7) becomes a second input
/// this function checks first.
pub fn from_pane(pane: Pane) -> FocusTarget {
    match pane {
        Pane::Explorer => FocusTarget::Explorer,
        Pane::Tabs => FocusTarget::Tabs,
        Pane::Editor => FocusTarget::Editor,
        Pane::Title => FocusTarget::Title,
    }
}

/// The resolved answer to "what is actually painted this frame" — the ONE
/// place that decision is made (`resolve`, below), so `App::set_focus`
/// never again has to trust `App::splits`' raw `shown` flags directly (the
/// shadow-state bug this module exists to close: `layout::geometry` can
/// drop the left column even while its `Split`s still say `shown`, e.g. a
/// column too short to fit either the Explorer or the tab rows). Every
/// other consumer that used to ask `app.splits.left.is_shown()` before
/// deciding whether a pane can take focus should ask a `LayoutMode`
/// instead.
///
/// `ExplorerOnly` is reserved for a future full-width Explorer — `resolve`
/// never produces it yet, so it costs `focusable` nothing today beyond one
/// extra match arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    /// The left column is painted; `explorer`/`tabs` are each independently
    /// whether THEIR OWN section within it has room this frame (mirrors
    /// `layout::Geometry::explorer_inner`/`tabs_inner`, which can each
    /// independently collapse while the column itself stays up).
    Split { explorer: bool, tabs: bool },
    /// Reserved for a future full-width Explorer; not reachable today.
    ExplorerOnly,
    /// The left column isn't painted this frame at all — either its own
    /// `Split` says hidden, or the frame is too small to fit it even though
    /// the `Split` still says shown.
    EditorOnly,
}

impl LayoutMode {
    /// Resolves purely from the frame's live geometry (`layout::
    /// resolve_mode`, the same chokepoint `layout::geometry` itself decides
    /// visibility through) plus the raw `Split` flags — the ONE function
    /// this whole module's guarantee rests on.
    ///
    /// Guarded on an unsized frame (`frame_width`/`frame_height` still `0`,
    /// i.e. before the first `Msg::Resize` ever lands — `App::relayout` is
    /// documented as a no-op in exactly this state): `layout::geometry`
    /// would otherwise report every pane collapsed at a zero-area frame,
    /// which is a "not measured yet" state, not a real "nothing fits"
    /// verdict, so this falls back to trusting the raw flags instead of
    /// asking geometry a question it cannot yet answer meaningfully.
    pub fn resolve(app: &App) -> LayoutMode {
        if app.frame_width == 0 || app.frame_height == 0 {
            return if app.splits.left.is_shown() {
                LayoutMode::Split {
                    explorer: true,
                    tabs: true,
                }
            } else {
                LayoutMode::EditorOnly
            };
        }
        let area = Rect::new(0, 0, app.frame_width, app.frame_height);
        crate::layout::resolve_mode(area, app)
    }

    /// `Some(VisiblePane(pane))` exactly when `pane` is painted under this
    /// mode — the ONE way to mint a `VisiblePane`, so a caller can never
    /// hand `App::set_focus` a pane absent from the current mode; it has to
    /// name `None` and decide what to do instead. The title is orthogonal
    /// to the left column's own layout — it names the document showing in
    /// the center pane, painted in every mode this resolver can produce.
    pub fn focusable(self, pane: Pane) -> Option<VisiblePane> {
        let painted = match (self, pane) {
            (_, Pane::Title) => true,
            (LayoutMode::Split { explorer, .. }, Pane::Explorer) => explorer,
            (LayoutMode::Split { tabs, .. }, Pane::Tabs) => tabs,
            (LayoutMode::Split { .. }, Pane::Editor) => true,
            (LayoutMode::EditorOnly, Pane::Editor) => true,
            (LayoutMode::EditorOnly, Pane::Explorer | Pane::Tabs) => false,
            (LayoutMode::ExplorerOnly, Pane::Explorer | Pane::Tabs) => true,
            (LayoutMode::ExplorerOnly, Pane::Editor) => false,
        };
        painted.then_some(VisiblePane(pane))
    }

    /// The one pane every `LayoutMode` variant paints by construction —
    /// `Split`/`EditorOnly` always paint the Editor, `ExplorerOnly` always
    /// paints the Explorer (a full-width Explorer has to paint SOMETHING).
    /// `focus_or_default`'s fallback, kept as its own total function rather
    /// than folded into an `unwrap_or` so it can never itself name a pane
    /// `focusable` would refuse — the exhaustive test below pins that for
    /// every variant this match can ever grow.
    fn default_focus(self) -> VisiblePane {
        match self {
            LayoutMode::Split { .. } | LayoutMode::EditorOnly => VisiblePane(Pane::Editor),
            LayoutMode::ExplorerOnly => VisiblePane(Pane::Explorer),
        }
    }

    /// `pane` if it's painted this frame, else `default_focus` — the ONE
    /// fallback both the collapse command (`GlobalCommand::CollapseLeft`)
    /// and the splitter drag path already reached for, ad hoc, before this
    /// module existed (`App::set_focus_pane`/`reconcile`, below, are now
    /// their shared chokepoint).
    pub fn focus_or_default(self, pane: Pane) -> VisiblePane {
        self.focusable(pane).unwrap_or_else(|| self.default_focus())
    }
}

/// A `Pane` known to be painted under some `LayoutMode` at the moment it was
/// minted — the token `App::set_focus` requires instead of a bare `Pane`, so
/// "focus lands on a pane nobody can see" cannot compile. The inner field is
/// private to this module (not merely unexported) on purpose: only
/// `LayoutMode::focusable`/`default_focus`/`focus_or_default` may construct
/// one, so the guarantee cannot be bypassed by a caller outside this file
/// reaching for the tuple constructor directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisiblePane(Pane);

impl VisiblePane {
    pub fn pane(self) -> Pane {
        self.0
    }
}

impl App {
    /// Which chrome region owns the next keystroke — the sole read of
    /// `focus` from outside this module.
    pub fn focus(&self) -> Pane {
        self.focus
    }

    /// This frame's resolved `LayoutMode` — every focus transition below
    /// reads through here rather than `self.splits` directly.
    pub fn layout_mode(&self) -> LayoutMode {
        LayoutMode::resolve(self)
    }

    /// The single writer of a read-only refusal's status message: posts
    /// `read_only`'s wording and reports whether it refused — `focus_title`
    /// and `rename::begin` both call this instead of duplicating the check
    /// (`refocus_title`'s silent return on the same precondition is a
    /// re-focus, not a refusal, so it does not).
    pub fn refuse_if_read_only(&mut self, read_only: ReadOnly) -> bool {
        let Some(message) = read_only.refusal_message() else {
            return false;
        };
        self.set_status(message, crate::app::StatusSource::Other);
        true
    }

    /// Gains title focus, reseeding the field from the active document's own
    /// name and landing the cursor at the end. Needs no `Effects`: entering
    /// the title can never itself leave it, so there is nothing to commit on
    /// the way in. Refuses on a read-only document — there is nothing to
    /// rename, and a refusal there would otherwise hold focus hostage in a
    /// field that can never commit (the Help document is the case this
    /// removes rather than guards against).
    pub fn focus_title(&mut self) {
        if self.refuse_if_read_only(self.active_doc().read_only) {
            return;
        }
        // Title is painted in every mode this resolver can produce today
        // (see `LayoutMode::focusable`'s doc) — the check still runs, not
        // trusted implicitly, so a future `ExplorerOnly` that hides the
        // center pane gates this automatically, with nothing here to
        // remember to update.
        let Some(target) = self.layout_mode().focusable(Pane::Title) else {
            return;
        };
        let name = crate::title::name_for(self.active_doc());
        self.title.seed(&name);
        self.focus = target.pane();
    }

    /// Returns to the title WITHOUT reseeding — the failed-rename/dismissed-
    /// collision path, where the field already holds what the user typed and
    /// reseeding would resurrect the old name and discard their in-progress
    /// undo history.
    pub fn refocus_title(&mut self) {
        // The same read-only precondition `focus_title` enforces: an async
        // reply can land after the active document has changed under it,
        // and parking focus on a title that can never commit would hold the
        // user there until they found Escape.
        if self.active_doc().is_read_only() {
            return;
        }
        let Some(target) = self.layout_mode().focusable(Pane::Title) else {
            return;
        };
        self.focus = target.pane();
    }

    /// The one writer for every focus transition OTHER than gaining the
    /// title (`focus_title`/`refocus_title` above). Leaving the title runs
    /// `title::on_blur` first: a `Refused` commit vetoes the transition
    /// (focus stays put, the reason is already in the footer) but the caller
    /// is never blocked from doing whatever it was about to do next — that
    /// is what makes a repeated, idempotent blur safe here.
    ///
    /// Takes a `VisiblePane`, not a bare `Pane`: the only way to get one is
    /// `LayoutMode::focusable`/`focus_or_default`, so a caller naming a pane
    /// absent from the current mode cannot reach this function without first
    /// deciding what to do about that — it cannot compile a focus transition
    /// onto an invisible pane by omission.
    pub fn set_focus(&mut self, next: VisiblePane, effects: &mut Effects) {
        let next = next.pane();
        if self.focus == next {
            return;
        }
        if self.focus == Pane::Title
            && crate::title::on_blur(self, effects) == crate::rename::Commit::Refused
        {
            return;
        }
        // A live Explorer type-to-search doesn't survive a focus round-trip
        // (design: "leaving the Explorer ... -> search cleared") — the
        // exact leak Go's own wall-clock reset has, since nothing there
        // clears the buffer on blur either. `set_focus` is already the blur
        // chokepoint for the title (`title::on_blur` above), so this is the
        // one place every route off the Explorer funnels through, same
        // reasoning.
        if self.focus == Pane::Explorer && next != Pane::Explorer {
            crate::explorer_search::clear_search(self);
        }
        self.focus = next;
    }

    /// The ergonomic entry point every ordinary call site uses instead of
    /// minting a `VisiblePane` by hand: focuses `pane` if it's painted this
    /// frame, else falls back to `LayoutMode::default_focus`.
    /// `pane::handle_global_command`'s `FocusExplorer`/`FocusTabs` arms rely
    /// on this to land focus correctly even though they just mutated
    /// `self.splits` moments earlier — `layout_mode()` re-resolves fresh
    /// from the live `Split` state every call, never a cached snapshot.
    pub fn set_focus_pane(&mut self, pane: Pane, effects: &mut Effects) {
        let target = self.layout_mode().focus_or_default(pane);
        self.set_focus(target, effects);
    }

    /// The blur chokepoint in PREFIX position: every site that changes the
    /// active document calls this BEFORE the switch, then decides
    /// separately (and conditionally, wherever the switch itself can fail)
    /// where focus should land afterwards. A no-op unless the title
    /// actually holds focus.
    pub fn blur_title(&mut self, effects: &mut Effects) {
        if self.focus == Pane::Title {
            self.set_focus_pane(Pane::Editor, effects);
        }
    }
}

/// Redirects focus to the Editor if the pane that currently holds it just
/// stopped being painted this frame — the ONE reconciliation both the
/// collapse command (`GlobalCommand::CollapseLeft`, `pane.rs`) and the
/// splitter drag path (`commands/splitter.rs`) run through, so a drag that
/// hides the same section a keybinding would hide can never land focus
/// somewhere the command path wouldn't.
pub fn reconcile(app: &mut App, effects: &mut Effects) {
    if app.layout_mode().focusable(app.focus()).is_none() {
        app.set_focus_pane(app.focus(), effects);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn derives_from_each_pane() {
        assert_eq!(from_pane(Pane::Explorer), FocusTarget::Explorer);
        assert_eq!(from_pane(Pane::Tabs), FocusTarget::Tabs);
        assert_eq!(from_pane(Pane::Editor), FocusTarget::Editor);
        assert_eq!(from_pane(Pane::Title), FocusTarget::Title);
    }

    /// No `LayoutMode` this resolver can produce may ever call `Explorer`
    /// or `Tabs` focusable while also reporting the column not painted —
    /// the precise shape of the shadow-state bug this module exists to
    /// close.
    #[test]
    fn no_mode_makes_an_unpainted_pane_focusable() {
        let editor_only = LayoutMode::EditorOnly;
        assert!(editor_only.focusable(Pane::Explorer).is_none());
        assert!(editor_only.focusable(Pane::Tabs).is_none());
        assert!(editor_only.focusable(Pane::Editor).is_some());
        assert!(editor_only.focusable(Pane::Title).is_some());

        let split_collapsed = LayoutMode::Split {
            explorer: false,
            tabs: false,
        };
        assert!(split_collapsed.focusable(Pane::Explorer).is_none());
        assert!(split_collapsed.focusable(Pane::Tabs).is_none());
        assert!(split_collapsed.focusable(Pane::Editor).is_some());
    }

    /// `focus_or_default` never leaves a caller with nothing to focus: an
    /// unpainted target always resolves to `default_focus` instead.
    #[test]
    fn focus_or_default_falls_back_when_the_target_is_unpainted() {
        let mode = LayoutMode::EditorOnly;
        assert_eq!(mode.focus_or_default(Pane::Explorer).pane(), Pane::Editor);
        assert_eq!(mode.focus_or_default(Pane::Editor).pane(), Pane::Editor);
    }

    /// The load-bearing proof `focus_or_default`'s whole guarantee rests on:
    /// for every `LayoutMode` variant — `ExplorerOnly` included, though
    /// nothing produces it yet — and every `Pane`, the result is a pane
    /// `focusable` accepts under that SAME mode. Written as a loop over
    /// every variant, not a handful of examples, so it keeps holding when a
    /// later work package adds another `LayoutMode` or `Pane` variant: a
    /// fallback that ever named an unpainted pane (the exact defect
    /// `default_focus` replaced an `unwrap_or(VisiblePane(pane))` escape
    /// hatch to close) fails this test immediately, for that variant.
    #[test]
    fn focus_or_default_never_names_a_pane_its_own_mode_refuses() {
        let modes = [
            LayoutMode::Split {
                explorer: true,
                tabs: true,
            },
            LayoutMode::Split {
                explorer: false,
                tabs: true,
            },
            LayoutMode::Split {
                explorer: true,
                tabs: false,
            },
            LayoutMode::Split {
                explorer: false,
                tabs: false,
            },
            LayoutMode::ExplorerOnly,
            LayoutMode::EditorOnly,
        ];
        let panes = [Pane::Explorer, Pane::Tabs, Pane::Editor, Pane::Title];

        for mode in modes {
            for pane in panes {
                let target = mode.focus_or_default(pane);
                assert!(
                    mode.focusable(target.pane()).is_some(),
                    "{mode:?}.focus_or_default({pane:?}) produced {target:?}, \
                     which {mode:?}.focusable refuses"
                );
            }
        }
    }
}
