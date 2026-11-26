//! Data tab content (#252 fidelity sweep). Visio group order:
//! **External Data · Shape Data · Data Graphics · Display Options**
//! (everything M6-planned). Legend left for Insert ▸ Diagram Parts;
//! the two window toggles fold into their owning groups.

use super::{Action, Group, Icon, RibbonButton, TabCtx, big, grid, stack};

pub(crate) fn groups(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<Group> {
    vec![
        Group::new(s.msg_external_data(), external_data_cols(s, ctx)),
        Group::new(s.msg_shape_data(), shape_data_cols(s, ctx)),
        Group::new(s.msg_data_graphics(), data_graphics_cols(s, ctx)),
        Group::new(s.msg_display_options(), display_options_cols(s, ctx)),
    ]
}

fn external_data_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![
        big(
            RibbonButton::new(Icon::Open, s.msg_quick_import(), Action::DataImport)
                .enabled(ctx.has_doc)
                .tip(s.msg_data_import_tip()),
        ),
        grid(vec![
            vec![
                RibbonButton::new(Icon::Link, "", Action::DataLinkRow)
                    .alt(s.msg_link_data_to_shapes())
                    .enabled(ctx.has_sel && ctx.data_table)
                    .tip(s.msg_link_row_tip()),
                RibbonButton::new(Icon::Refresh, "", Action::DataRefresh)
                    .alt(s.msg_refresh_all())
                    .enabled(ctx.has_doc)
                    .tip(s.msg_refresh_all_tip()),
            ],
            vec![
                RibbonButton::new(Icon::Shapes, "", Action::DataImport)
                    .alt(s.msg_custom_import())
                    .enabled(ctx.has_doc)
                    .tip(s.msg_data_import_tip()),
                RibbonButton::new(Icon::Magnet, "", Action::DataAutoLink)
                    .alt(s.msg_automatically_link())
                    .enabled(ctx.data_table)
                    .tip(s.msg_auto_link_tip()),
            ],
        ]),
        stack(vec![RibbonButton::new(
            Icon::Units,
            s.msg_external_data_window(),
            Action::DataWindow,
        )
        .enabled(ctx.data_table)
        .tip(s.msg_external_data_tip())]),
    ]
}

fn shape_data_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![
        big(
            RibbonButton::new(Icon::ShapeSheet, s.msg_shape_data_window(), Action::ShapeDataWindow)
                .enabled(ctx.has_sel)
                .tip(s.msg_shape_data_window()),
        ),
        stack(vec![RibbonButton::new(
            Icon::Field,
            s.msg_define_shape_data(),
            Action::Macros,
        )
        .enabled(false)
        .tip(s.msg_define_shape_data_m6())]),
    ]
}

fn data_graphics_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![
        big(
            RibbonButton::new(Icon::DataGraphic, s.msg_data_graphics(), Action::DataGraphics)
                .enabled(ctx.has_doc)
                .tip(s.msg_data_graphics_tip()),
        ),
        grid(vec![
            vec![
                RibbonButton::new(Icon::Effects, "", Action::DataGraphics)
                    .alt(s.msg_create_new_data_graphic())
                    .enabled(ctx.has_doc)
                    .tip(s.msg_data_graphics_tip()),
                RibbonButton::new(Icon::Pen, "", Action::DataGraphics)
                    .alt(s.msg_edit_data_graphic())
                    .enabled(ctx.has_doc)
                    .tip(s.msg_data_graphics_tip()),
            ],
        ]),
    ]
}

fn display_options_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![stack(vec![RibbonButton::new(
        Icon::Check,
        s.msg_display_data_graphics(),
        Action::Macros,
    )
    .enabled(false)
    .tip(s.msg_show_hide_data_graphics_on_the_p())])]
}
