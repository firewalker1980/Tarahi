//! Container Tools contextual tab (#156, #252). Reshaped against the
//! reference shot that landed on #252 (2026-11-09): Visio paints
//! **Size · Container Styles · Heading Style · Membership** — Margins
//! / Fit to Contents / Automatic Resize placeholders, the SDCONT
//! gallery, the heading-style gallery, and the membership set with a
//! wired Disband Container.

use super::{Action, Group, Icon, RibbonButton, TabCtx, big, grid, stack};

/// #156: the SDCONT container style gallery — the shipped Visio 2016
/// set, in gallery order (Analysis/container-stencil-sdcont.md; the
/// masters live in the embedded `sdcont` stencil). Shared with the
/// Container Styles gallery popup in `app.rs`.
pub(crate) const CONTAINER_STYLES: [&str; 14] = [
    "Plain",
    "Notch",
    "Waves",
    "Wire",
    "Belt",
    "Classic",
    "Full sail",
    "Corners",
    "Alternating",
    "Banner",
    "Picture Frame",
    "Snapshot",
    "Intersection",
    "Translucent",
];

pub(crate) fn groups(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<Group> {
    vec![
        Group::new(s.msg_size(), size_cols(s, ctx)),
        Group::new(s.msg_container_styles(), styles_grid(s, ctx)),
        Group::new(s.msg_heading_style(), heading_cols(s, ctx)),
        Group::new(s.msg_membership(), membership_cols(s, ctx)),
    ]
}

fn size_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![
        big(
            RibbonButton::new(Icon::Grid, s.msg_margins(), Action::ContainerMenu(0))
                .enabled(ctx.sel_container)
                .split()
                .tip(s.msg_container_margins_tip()),
        ),
        stack(vec![
            RibbonButton::new(Icon::Fit, s.msg_fit_to_contents(), Action::ContainerFit)
                .enabled(ctx.sel_container)
                .tip(s.msg_fit_to_contents_tip()),
            RibbonButton::new(Icon::Fit, s.msg_automatic_resize(), Action::ContainerMenu(1))
                .enabled(ctx.sel_container)
                .split()
                .tip(s.msg_auto_resize_tip()),
        ]),
    ]
}

fn styles_grid(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![grid(vec![
        CONTAINER_STYLES[..7]
            .iter()
            .map(|n| {
                RibbonButton::new(Icon::Container, *n, Action::ContainerStyle(n))
                    .tip(s.msg_wrap_the_selection_in_this_conta())
            })
            .collect::<Vec<_>>(),
        CONTAINER_STYLES[7..]
            .iter()
            .map(|n| {
                RibbonButton::new(Icon::Container, *n, Action::ContainerStyle(n))
                    .tip(s.msg_wrap_the_selection_in_this_conta())
            })
            .collect::<Vec<_>>(),
    ])]
}

fn heading_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![big(
        RibbonButton::new(Icon::FontA, s.msg_heading_style(), Action::ContainerHeading)
            .enabled(ctx.sel_container)
            .split()
            .tip(s.msg_heading_style_tip()),
    )]
}

fn membership_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![
        grid(vec![
            vec![
                RibbonButton::new(Icon::Container, s.msg_highlight_members(), Action::ContainerHighlight)
                    .selected(ctx.container_highlight)
                    .tip(s.msg_tint_the_container_s_members_whi()),
                RibbonButton::new(Icon::Lock, "", Action::ContainerLock)
                    .alt(s.msg_lock_container())
                    .enabled(ctx.sel_container)
                    .selected(ctx.container_locked)
                    .tip(s.msg_lock_container_tip()),
            ],
            vec![
                RibbonButton::new(Icon::Shapes, "", Action::ContainerSelect)
                    .alt(s.msg_select_contents())
                    .enabled(ctx.sel_container)
                    .tip(s.msg_select_contents_tip()),
                RibbonButton::new(Icon::Delete, "", Action::ContainerDisband)
                    .alt(s.msg_disband_container())
                    .enabled(ctx.sel_container)
                    .tip(s.msg_release_the_members_and_delete_t()),
            ],
        ]),
    ]
}
