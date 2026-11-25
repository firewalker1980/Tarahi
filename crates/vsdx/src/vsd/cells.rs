//! Record layouts of the v11 binary format: where each ShapeSheet
//! cell sits inside its record. Offsets were scored against Visio's
//! own .vss → .vssx conversions of ~100 vendor stencils (agreement on
//! hundreds of nonzero samples per cell); fields without that evidence
//! are left out rather than guessed.
//!
//! A unit-carrying number is one unit byte followed by an f64, so the
//! offsets below point at the f64 and the unit code sits at `off - 1`.

#[derive(Clone, Copy, Debug)]
pub enum Enc {
    F64,
    U8,
    U16,
    /// one bit of a flag byte
    Bit(u8),
    /// color: palette index byte, then R, G, B
    Color,
    /// transparency byte, 0..=255 → 0..1 (in 1/256 steps)
    Trans,
    /// f64 value unless its unit byte is 0xE8 (string kept in the formula)
    Num,
}
use Enc::*;

/// Shape/style header (level-0 record): parent, master, master shape
/// and the three style ids, u32 each.
pub const HDR_PARENT: usize = 10;
pub const HDR_MASTER: usize = 18;
pub const HDR_MASTER_SHAPE: usize = 26;
pub const HDR_FILL_STYLE: usize = 34;
pub const HDR_LINE_STYLE: usize = 42;
pub const HDR_TEXT_STYLE: usize = 50;

pub struct CellDef {
    pub t: u32,
    pub off: usize,
    pub enc: Enc,
    pub name: &'static str,
}

const fn c(t: u32, off: usize, enc: Enc, name: &'static str) -> CellDef {
    CellDef { t, off, enc, name }
}

/// Shape-level (single-row section) cells.
pub const CELLS: &[CellDef] = &[
    // XForm
    c(0x9b, 1, F64, "PinX"),
    c(0x9b, 10, F64, "PinY"),
    c(0x9b, 19, F64, "Width"),
    c(0x9b, 28, F64, "Height"),
    c(0x9b, 37, F64, "LocPinX"),
    c(0x9b, 46, F64, "LocPinY"),
    c(0x9b, 55, F64, "Angle"),
    c(0x9b, 63, U8, "FlipX"),
    c(0x9b, 64, U8, "FlipY"),
    c(0x9b, 65, U8, "ResizeMode"),
    // TextXForm
    c(0x9c, 1, F64, "TxtPinX"),
    c(0x9c, 10, F64, "TxtPinY"),
    c(0x9c, 19, F64, "TxtWidth"),
    c(0x9c, 28, F64, "TxtHeight"),
    c(0x9c, 37, F64, "TxtLocPinX"),
    c(0x9c, 46, F64, "TxtLocPinY"),
    c(0x9c, 55, F64, "TxtAngle"),
    // XForm1D
    c(0x9d, 1, F64, "BeginX"),
    c(0x9d, 10, F64, "BeginY"),
    c(0x9d, 19, F64, "EndX"),
    c(0x9d, 28, F64, "EndY"),
    // Line
    c(0x85, 1, F64, "LineWeight"),
    c(0x85, 9, Color, "LineColor"),
    c(0x85, 13, Trans, "LineColorTrans"),
    c(0x85, 14, U8, "LinePattern"),
    c(0x85, 16, F64, "Rounding"),
    c(0x85, 24, U8, "EndArrowSize"),
    c(0x85, 25, U8, "BeginArrow"),
    c(0x85, 26, U8, "EndArrow"),
    c(0x85, 27, U8, "LineCap"),
    c(0x85, 28, U8, "BeginArrowSize"),
    // Fill (colors are indices into the document color table)
    c(0x86, 0, Color, "FillForegnd"),
    c(0x86, 5, Color, "FillBkgnd"),
    c(0x86, 10, U8, "FillPattern"),
    c(0x86, 11, Color, "ShdwForegnd"),
    c(0x86, 16, Color, "ShdwBkgnd"),
    c(0x86, 21, U8, "ShdwPattern"),
    c(0x86, 4, Trans, "FillForegndTrans"),
    c(0x86, 9, Trans, "FillBkgndTrans"),
    c(0x86, 15, Trans, "ShdwForegndTrans"),
    c(0x86, 20, Trans, "ShdwBkgndTrans"),
    c(0x86, 22, U8, "ShapeShdwType"),
    c(0x86, 24, F64, "ShapeShdwOffsetX"),
    c(0x86, 33, F64, "ShapeShdwOffsetY"),
    c(0x86, 42, F64, "ShapeShdwObliqueAngle"),
    c(0x86, 50, F64, "ShapeShdwScaleFactor"),
    // TextBlock
    c(0x87, 1, F64, "LeftMargin"),
    c(0x87, 10, F64, "RightMargin"),
    c(0x87, 19, F64, "TopMargin"),
    c(0x87, 28, F64, "BottomMargin"),
    c(0x87, 36, U8, "VerticalAlign"),
    c(0x87, 37, Color, "TextBkgnd"),
    c(0x87, 41, Trans, "TextBkgndTrans"),
    c(0x87, 43, F64, "DefaultTabStop"),
    // Foreign image box
    c(0x98, 1, F64, "ImgOffsetX"),
    c(0x98, 10, F64, "ImgOffsetY"),
    c(0x98, 19, F64, "ImgWidth"),
    c(0x98, 28, F64, "ImgHeight"),
    // Protection
    c(0xa0, 0, U8, "LockWidth"),
    c(0xa0, 1, U8, "LockHeight"),
    c(0xa0, 4, U8, "LockAspect"),
    c(0xa0, 10, U8, "LockVtxEdit"),
    // Misc: flag bits of byte 0, object type, drop scale
    c(0xa4, 0, Bit(0), "NoObjHandles"),
    c(0xa4, 0, Bit(1), "NonPrinting"),
    c(0xa4, 0, Bit(2), "NoCtlHandles"),
    c(0xa4, 0, Bit(3), "NoAlignBox"),
    c(0xa4, 0, Bit(4), "UpdateAlignBox"),
    c(0xa4, 0, Bit(5), "HideText"),
    c(0xa4, 6, U8, "ObjType"),
    c(0xa4, 24, F64, "DropOnPageScale"),
    // Page properties (page/master sheet)
    c(0x92, 1, F64, "PageWidth"),
    c(0x92, 10, F64, "PageHeight"),
    c(0x92, 19, F64, "ShdwOffsetX"),
    c(0x92, 28, F64, "ShdwOffsetY"),
    c(0x92, 37, F64, "PageScale"),
    c(0x92, 46, F64, "DrawingScale"),
];

/// Section list record type → section name.
pub const SECTIONS: &[(u32, &str)] = &[
    (0x6c, "Geometry"),
    (0x69, "Character"),
    (0x6a, "Paragraph"),
    (0x71, "Connection"),
    (0x70, "Control"),
    (0x64, "Scratch"),
    (0x67, "User"),
    (0x6d, "Property"),
    (0x6e, "Actions"),
    (0x73, "Hyperlink"),
    (0x66, "Field"),
];

pub struct RowDef {
    pub t: u32,
    /// The section's header row (cells go on the Section itself).
    pub header: bool,
    pub row_t: Option<&'static str>,
    pub cells: &'static [(&'static str, usize, Enc)],
}

const XY: &[(&str, usize, Enc)] = &[("X", 1, F64), ("Y", 10, F64)];
const XYABCD: &[(&str, usize, Enc)] = &[
    ("X", 1, F64),
    ("Y", 10, F64),
    ("A", 19, F64),
    ("B", 28, F64),
    ("C", 37, F64),
    ("D", 46, F64),
];

pub const ROWS: &[RowDef] = &[
    RowDef {
        t: 0x89,
        header: true,
        row_t: None,
        cells: &[
            ("NoFill", 0, Bit(0)),
            ("NoLine", 0, Bit(1)),
            ("NoShow", 0, Bit(2)),
            ("NoSnap", 0, Bit(3)),
        ],
    },
    RowDef {
        t: 0x8a,
        header: false,
        row_t: Some("MoveTo"),
        cells: XY,
    },
    RowDef {
        t: 0x8b,
        header: false,
        row_t: Some("LineTo"),
        cells: XY,
    },
    RowDef {
        t: 0x8c,
        header: false,
        row_t: Some("ArcTo"),
        cells: &[("X", 1, F64), ("Y", 10, F64), ("A", 19, F64)],
    },
    RowDef {
        t: 0x8f,
        header: false,
        row_t: Some("Ellipse"),
        cells: XYABCD,
    },
    RowDef {
        t: 0x90,
        header: false,
        row_t: Some("EllipticalArcTo"),
        cells: XYABCD,
    },
    RowDef {
        t: 0xc1,
        header: false,
        row_t: Some("PolylineTo"),
        cells: XY,
    },
    RowDef {
        t: 0xc3,
        header: false,
        row_t: Some("NURBSTo"),
        cells: XYABCD,
    },
    RowDef {
        t: 0x94,
        header: false,
        row_t: None,
        cells: &[
            ("Font", 4, U16),
            ("Color", 6, Color),
            ("ColorTrans", 10, Trans),
            ("Style", 11, U8),
            ("Pos", 13, U8),
            ("Size", 18, F64),
        ],
    },
    RowDef {
        t: 0x95,
        header: false,
        row_t: None,
        cells: &[
            ("IndFirst", 5, F64),
            ("IndLeft", 14, F64),
            ("IndRight", 23, F64),
            ("SpLine", 32, F64),
            ("HorzAlign", 58, U8),
        ],
    },
    RowDef {
        t: 0x99,
        header: false,
        row_t: Some("Connection"),
        cells: &[
            ("X", 1, F64),
            ("Y", 10, F64),
            ("DirX", 19, F64),
            ("DirY", 28, F64),
            ("Type", 36, U8),
        ],
    },
    RowDef {
        t: 0xaa,
        header: false,
        row_t: None,
        cells: &[
            ("X", 1, F64),
            ("Y", 10, F64),
            ("XDyn", 19, F64),
            ("YDyn", 28, F64),
        ],
    },
    RowDef {
        t: 0x9e,
        header: false,
        row_t: None,
        cells: XYABCD,
    },
    // Text field: Value/Format come from formula slots 0/2
    RowDef {
        t: 0xa1,
        header: false,
        row_t: None,
        cells: &[("UICod", 29, U8)],
    },
    // User / Prop: a value at 1 unless held as a string formula (unit
    // byte 0xE8); Prop's data type at 25. Other cells come from formulas.
    RowDef {
        t: 0xb4,
        header: false,
        row_t: None,
        cells: &[("Value", 1, Num)],
    },
    RowDef {
        t: 0xb6,
        header: false,
        row_t: None,
        cells: &[("Value", 1, Num), ("Type", 25, U8)],
    },
    RowDef {
        t: 0xa9,
        header: false,
        row_t: None,
        cells: &[],
    },
    RowDef {
        t: 0xc4,
        header: false,
        row_t: None,
        cells: &[],
    },
];

/// Unit code (the byte before an f64) → the `U` attribute Visio writes.
pub fn unit(code: u8) -> Option<&'static str> {
    Some(match code {
        0x41 => "IN",
        0x32 => "PT",
        0x45 => "CM",
        0x46 => "MM",
        0x51 => "DEG",
        _ => return None,
    })
}
