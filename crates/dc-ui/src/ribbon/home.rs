//! Home tab content (#252 fidelity sweep). Visio group order:
//! **Clipboard · Font · Paragraph · Tools · Shape Styles · Arrange ·
//! Editing**. The Select cluster (Select All / Select by Type /
//! Lasso) rides beside Clipboard pending Visio's Select split-button
//! dropdown; Sub/Superscript, Clear Formatting and Line Spacing moved
//! to the text dialog's tabs per the #252 disposition. Compact tiers
//! (icon-only tool grid, 2-wide Arrange clusters) keep the WIRED
//! groups inside a 1280 px window; the all-placeholder Editing group
//! is last and may clip on narrow windows, as Visio's own collapsed
//! tier does.

use super::{Action, Group, Icon, RibbonButton, TabCtx, big, grid, stack};
use crate::app::Tool;

pub(crate) fn groups(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<Group> {
    let has_sel = ctx.has_sel;
    let multi_sel = ctx.multi_sel;
    let tri_sel = ctx.tri_sel;
    vec![
        Group::new(s.msg_clipboard(), clipboard_cols(s, ctx, has_sel)),
        Group::new(s.msg_font(), vec![font_grid(s, ctx, has_sel)]).with_launcher(),
        Group::new(s.msg_paragraph(), vec![paragraph_grid(s, ctx)]).with_launcher(),
        Group::new(s.msg_tools(), tools_cols(s, ctx)),
        Group::new(s.msg_shape_styles(), shape_styles_cols(s, has_sel)),
        Group::new(s.msg_arrange(), arrange_cols(s, ctx, multi_sel, tri_sel)),
        // #252: Visio's Editing group — Find/Replace/Clear are
        // placeholders (no backend yet); Select stays beside
        // Clipboard until the Select dropdown lands.
        Group::new(
            s.msg_editing(),
            vec![grid(vec![
                vec![
                    RibbonButton::new(Icon::Find, "", Action::Find)
                        .alt(s.msg_find())
                        .enabled(ctx.has_doc)
                        .tip(s.msg_find_replace_tip()),
                    RibbonButton::new(Icon::FindReplace, "", Action::Replace)
                        .alt(s.msg_replace())
                        .enabled(ctx.has_doc)
                        .tip(s.msg_find_replace_tip()),
                ],
                vec![
                    RibbonButton::new(Icon::SelectionPane, "", Action::Layers)
                        .alt(s.msg_layers())
                        .enabled(ctx.has_doc)
                        .tip(s.msg_layer_properties_tip()),
                    RibbonButton::new(Icon::Eraser, "", Action::ClearFormatting)
                        .alt(s.msg_clear())
                        .enabled(ctx.has_sel)
                        .tip(s.msg_clear_formatting_tip()),
                ],
            ])],
        ),
    ]
}

fn clipboard_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx, has_sel: bool) -> Vec<super::Col> {
    vec![
        big(
            RibbonButton::new(Icon::Paste, s.msg_paste(), Action::Paste)
                .enabled(ctx.has_doc)
                .tip(s.msg_paste()),
        ),
        grid(vec![
            vec![
                RibbonButton::new(Icon::Cut, "", Action::Cut)
                    .alt(s.msg_cut())
                    .enabled(has_sel)
                    .tip(s.msg_cut()),
                RibbonButton::new(Icon::Copy, "", Action::Copy)
                    .alt(s.msg_copy())
                    .enabled(has_sel)
                    .tip(s.msg_copy()),
            ],
            vec![RibbonButton::new(
                Icon::Painter,
                s.msg_format_painter(),
                Action::FormatPainter,
            )
            .enabled(has_sel)
            .selected(ctx.painter_armed)
            .tip(
                "Copy the selection's formatting to the next shape you click; \
             double-click to keep painting until Esc",
            )],
        ]),
        // Visio folds these into the Editing group's Select dropdown;
        // they stay a painted cluster beside Clipboard until that
        // dropdown lands (#110: Select All + Select by Type +
        // URS-1160, plus the freehand-trail Lasso).
        grid(vec![
            vec![
                RibbonButton::new(Icon::Shapes, "", Action::SelectAll)
                    .alt(s.msg_select_all())
                    .tip(s.msg_select_every_shape_on_the_page_c()),
                RibbonButton::new(Icon::Shapes, "", Action::SelectByType)
                    .alt(s.msg_select_by_type_menu())
                    .selected(ctx.selby_open)
                    .tip(s.msg_select_shapes_by_type_master_or()),
            ],
            vec![RibbonButton::new(Icon::Lasso, "", Action::LassoTool)
                .alt(s.msg_lasso_select())
                .selected(ctx.tool == Tool::Lasso)
                .tip(
                    "Draw a freehand trail; shapes the trail touches or encloses \
                 are selected (Esc to finish)",
                )],
        ]),
    ]
}

fn font_grid(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx, has_sel: bool) -> super::Col {
    grid(vec![
        vec![
            RibbonButton::new(Icon::FontA, ctx.font_name.clone(), Action::FontMenu)
                .alt(s.msg_font())
                .combo()
                .width(96.0)
                .enabled(has_sel)
                .tip(s.msg_font()),
            RibbonButton::new(Icon::FontA, ctx.font_size.clone(), Action::FontSizeMenu)
                .alt(s.msg_size_pt())
                .combo()
                .width(40.0)
                .enabled(has_sel)
                .tip(s.msg_size_pt()),
            RibbonButton::new(Icon::Grow, "", Action::FontGrow)
                .alt(s.msg_font_increase())
                .enabled(has_sel)
                .tip(s.msg_font_increase_tip()),
            RibbonButton::new(Icon::Shrink, "", Action::FontShrink)
                .alt(s.msg_font_decrease())
                .enabled(has_sel)
                .tip(s.msg_font_decrease_tip()),
        ],
        vec![
            RibbonButton::new(Icon::Bold, "", Action::Bold)
                .alt(s.msg_bold())
                .enabled(has_sel)
                .selected(has_sel && ctx.char_style & 1 != 0)
                .tip(s.msg_bold()),
            RibbonButton::new(Icon::Italic, "", Action::Italic)
                .alt(s.msg_italic())
                .enabled(has_sel)
                .selected(has_sel && ctx.char_style & 2 != 0)
                .tip(s.msg_italic()),
            RibbonButton::new(Icon::Underline, "", Action::Underline)
                .alt(s.msg_underline())
                .enabled(has_sel)
                .selected(has_sel && ctx.char_style & 4 != 0)
                .tip(s.msg_underline()),
            RibbonButton::new(Icon::Strike, "", Action::Strike)
                .alt(s.msg_strikethrough())
                .enabled(has_sel)
                .selected(has_sel && ctx.strike)
                .tip(s.msg_strikethrough()),
            // #150: wired to the shared palette like Fill/Line — a
            // pick writes the Character row-0 Color cell across the
            // selection (#73's ColorTarget::Text path).
            RibbonButton::new(Icon::TextColor, "", Action::TextColorMenu)
                .alt(s.msg_text_color())
                .enabled(has_sel)
                .tip(s.msg_text_color_tip()),
        ],
    ])
}

fn paragraph_grid(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> super::Col {
    grid(vec![
        vec![
            RibbonButton::new(Icon::Align, "", Action::TextAlignLeft)
                .alt(s.msg_text_align_left())
                .enabled(ctx.has_sel)
                .selected(ctx.has_sel && ctx.horz_align == 0)
                .tip(s.msg_text_align_left()),
            RibbonButton::new(Icon::AlignCenter, "", Action::TextAlignCenter)
                .alt(s.msg_align_center())
                .enabled(ctx.has_sel)
                .selected(ctx.has_sel && ctx.horz_align == 1)
                .tip(s.msg_align_center()),
            RibbonButton::new(Icon::AlignRight, "", Action::TextAlignRight)
                .alt(s.msg_text_align_right())
                .enabled(ctx.has_sel)
                .selected(ctx.has_sel && ctx.horz_align == 2)
                .tip(s.msg_text_align_right()),
            RibbonButton::new(Icon::Justify, "", Action::TextJustify)
                .alt(s.msg_justify())
                .enabled(ctx.has_sel)
                .selected(ctx.has_sel && ctx.horz_align == 3)
                .tip(s.msg_justify()),
        ],
        vec![
            RibbonButton::new(Icon::Bullets, "", Action::Bullets)
                .alt(s.msg_bullets())
                .enabled(ctx.has_sel)
                .selected(ctx.has_sel && ctx.bullet)
                .tip(s.msg_bullets()),
            RibbonButton::new(Icon::IndentLess, "", Action::Indent(false))
                .alt(s.msg_decrease_indent())
                .enabled(ctx.has_sel)
                .tip(s.msg_decrease_indent()),
            RibbonButton::new(Icon::IndentMore, "", Action::Indent(true))
                .alt(s.msg_increase_indent())
                .enabled(ctx.has_sel)
                .tip(s.msg_increase_indent()),
        ],
    ])
}

fn tools_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx) -> Vec<super::Col> {
    vec![
        stack(vec![
            RibbonButton::new(Icon::Pointer, "", Action::PointerTool)
                .alt(s.msg_pointer_tool())
                .selected(ctx.tool == Tool::Pointer)
                .tip(s.msg_pointer_tool_select_and_drag_esc()),
            RibbonButton::new(Icon::Connector, "", Action::ConnectorTool)
                .alt(s.msg_connector())
                .selected(ctx.tool == Tool::Connector)
                .tip(s.msg_connector_tool_status()),
        ]),
        stack(vec![
            RibbonButton::new(Icon::ConnPoint, "", Action::ConnPointTool)
                .alt(s.msg_conn_point())
                .selected(ctx.tool == Tool::ConnectionPoint)
                .tip(
                    "Connection Point tool: click a shape to add a connection \
                 point; click one to select it (Delete removes)",
                ),
            RibbonButton::new(Icon::Pen, "", Action::FreeformTool)
                .alt(s.msg_freeform())
                .selected(ctx.tool == Tool::Freeform)
                .tip(
                    "Freeform tool (#132): drag on the page to draw freehand \
                 geometry — the stroke becomes an editable shape (Esc finishes)",
                ),
        ]),
        // #283: Visio's pan affordance as a discoverable tool — the
        // two-button-mouse/trackpad case the shell never covered.
        stack(vec![
            RibbonButton::new(Icon::Hand, "", Action::PanTool)
                .alt(s.msg_pan())
                .selected(ctx.tool == Tool::Pan)
                .tip(s.msg_pan_tip()),
        ]),
        // Visio's shape-drawing tools, compact icon tier (planned).
        grid(vec![
            vec![
                RibbonButton::new(Icon::TextTool, "", Action::DrawTool(crate::drawtools::Kind::Text))
                    .alt(s.msg_text_tool())
                    .enabled(ctx.has_doc)
                    .selected(ctx.tool == crate::app::Tool::Draw(crate::drawtools::Kind::Text))
                    .tip(s.msg_text_tool_tip()),
                RibbonButton::new(Icon::RectTool, "", Action::DrawTool(crate::drawtools::Kind::Rect))
                    .alt(s.msg_rectangle_tool())
                    .enabled(ctx.has_doc)
                    .selected(ctx.tool == crate::app::Tool::Draw(crate::drawtools::Kind::Rect))
                    .tip(s.msg_rectangle_tool_tip()),
            ],
            vec![
                RibbonButton::new(Icon::EllipseTool, "", Action::DrawTool(crate::drawtools::Kind::Ellipse))
                    .alt(s.msg_ellipse_tool())
                    .enabled(ctx.has_doc)
                    .selected(ctx.tool == crate::app::Tool::Draw(crate::drawtools::Kind::Ellipse))
                    .tip(s.msg_ellipse_tool_tip()),
                RibbonButton::new(Icon::LineTool, "", Action::DrawTool(crate::drawtools::Kind::Line))
                    .alt(s.msg_line_tool())
                    .enabled(ctx.has_doc)
                    .selected(ctx.tool == crate::app::Tool::Draw(crate::drawtools::Kind::Line))
                    .tip(s.msg_line_tool_tip()),
            ],
            vec![
                RibbonButton::new(Icon::Stamp, "", Action::StampTool)
                    .alt(s.msg_stamp_tool())
                    .enabled(ctx.has_doc)
                    .selected(ctx.stamp)
                    .tip(s.msg_stamp_tool_tip()),
            ],
        ]),
    ]
}

fn shape_styles_cols(s: &'static crate::l10n::L10nLanguage, has_sel: bool) -> Vec<super::Col> {
    vec![
        big(
            RibbonButton::new(Icon::Theme, s.msg_quick_styles(), Action::Macros)
                .enabled(false)
                .tip(s.msg_style_gallery_planned()),
        ),
        stack(vec![
            RibbonButton::new(Icon::Bucket, s.msg_fill(), Action::FillMenu)
                .enabled(has_sel)
                .tip(s.msg_shape_fill_color()),
            RibbonButton::new(Icon::Pen, s.msg_line(), Action::LineMenu)
                .enabled(has_sel)
                .tip(s.msg_line_color_label()),
            RibbonButton::new(Icon::Effects, s.msg_effects(), Action::Macros)
                .enabled(false)
                .tip(s.msg_shape_effects_planned()),
        ]),
    ]
}

fn arrange_cols(s: &'static crate::l10n::L10nLanguage, ctx: &TabCtx, multi_sel: bool, tri_sel: bool) -> Vec<super::Col> {
    vec![
        stack(vec![
            RibbonButton::new(Icon::BringFront, "", Action::BringForward)
                .alt(s.msg_bring_forward())
                .enabled(ctx.has_sel)
                .tip(s.msg_bring_forward()),
            RibbonButton::new(Icon::SendBack, "", Action::SendBackward)
                .alt(s.msg_send_backward())
                .enabled(ctx.has_sel)
                .tip(s.msg_send_backward()),
        ]),
        grid(vec![
            vec![
                RibbonButton::new(Icon::Group, "", Action::Group)
                    .alt(s.msg_group())
                    .enabled(multi_sel)
                    .tip(s.msg_group_the_selection_one_undo_ste()),
                RibbonButton::new(Icon::Group, "", Action::Ungroup)
                    .alt(s.msg_ungroup())
                    .enabled(ctx.sel_group)
                    .tip(s.msg_ungroup_the_selected_group_s()),
            ],
            vec![
                RibbonButton::new(Icon::Rotate, "", Action::RotateLeft)
                    .alt(s.msg_rotate_left())
                    .enabled(ctx.has_sel)
                    .tip(s.msg_rotate_the_selection_90_counter()),
                RibbonButton::new(Icon::Rotate, "", Action::RotateRight)
                    .alt(s.msg_rotate_right())
                    .enabled(ctx.has_sel)
                    .tip(s.msg_rotate_the_selection_90_clockwis()),
            ],
            vec![
                RibbonButton::new(Icon::SelectionPane, "", Action::SelectionPane)
                    .alt(s.msg_selection_pane())
                    .enabled(ctx.has_doc)
                    .tip(s.msg_selection_pane_tip()),
                RibbonButton::new(Icon::Check, "", Action::AlignToPage)
                    .alt(s.msg_to_page())
                    .selected(ctx.align_to_page)
                    .tip(s.msg_align_against_the_page_instead_o()),
            ],
        ]),
        grid(vec![
            vec![
                RibbonButton::new(Icon::Align, "", Action::AlignLeft)
                    .alt(s.msg_align_left())
                    .enabled(multi_sel)
                    .tip(s.msg_align_left_edges_first_selected()),
                RibbonButton::new(Icon::AlignCenter, "", Action::AlignHCenter)
                    .alt(s.msg_center())
                    .enabled(multi_sel)
                    .tip(s.msg_align_vertical_centers()),
                RibbonButton::new(Icon::AlignRight, "", Action::AlignRight)
                    .alt(s.msg_align_right())
                    .enabled(multi_sel)
                    .tip(s.msg_align_right_edges()),
            ],
            vec![
                RibbonButton::new(Icon::AlignTop, "", Action::AlignTop)
                    .alt(s.msg_align_top())
                    .enabled(multi_sel)
                    .tip(s.msg_align_top_edges()),
                RibbonButton::new(Icon::AlignMiddle, "", Action::AlignVCenter)
                    .alt(s.msg_middle())
                    .enabled(multi_sel)
                    .tip(s.msg_align_horizontal_middles()),
                RibbonButton::new(Icon::AlignBottom, "", Action::AlignBottom)
                    .alt(s.msg_align_bottom())
                    .enabled(multi_sel)
                    .tip(s.msg_align_bottom_edges()),
            ],
            vec![
                RibbonButton::new(Icon::DistH, "", Action::DistributeH)
                    .alt(s.msg_dist_h())
                    .enabled(tri_sel)
                    .tip(s.msg_even_horizontal_spacing_3_shapes()),
                RibbonButton::new(Icon::DistV, "", Action::DistributeV)
                    .alt(s.msg_dist_v())
                    .enabled(tri_sel)
                    .tip(s.msg_even_vertical_spacing_3_shapes()),
            ],
        ]),
    ]
}
