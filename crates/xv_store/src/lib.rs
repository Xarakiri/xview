//! Persistence: atomic writes, settings, and per-symbol drawings files.
//! Corrupt files are moved aside, never overwritten.

mod atomic;
mod drawings;
mod settings;
mod templates;

pub use atomic::{data_dir, write_atomic};
pub use drawings::{LoadedDrawings, drawings_path, load_drawings, save_drawings};
pub use settings::{MagnetSetting, Settings, ThemeMode, WindowRect, load_settings, save_settings};
pub use templates::{
    LoadedTemplates, NamedTemplate, TemplatesFile, load_templates, push_custom_color,
    save_templates, templates_path,
};
