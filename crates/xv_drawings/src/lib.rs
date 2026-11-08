mod file;
mod hit;
mod model;
mod set;
mod tool;

pub use file::{DrawingsError, DrawingsFile, FILE_VERSION, from_json, to_json};
pub use hit::{HIT_TOLERANCE, Hit, Projector, handles, hit_test};
pub use model::{
    Anchor, Drawing, DrawingId, DrawingKind, Extend, HAlign, LineEnd, LineStyle, PALETTE, Range,
    StatsMode, StatsPos, Style, TEXT_COLOR, TextStyle, VAlign, Visibility,
};
pub use set::DrawingSet;
pub use tool::{PlaceResult, Tool, place_click, tool_of};
