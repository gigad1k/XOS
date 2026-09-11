//! The XOS palette, as terminal colours.
//!
//! Three semantic hues exist and each means exactly one thing: green is local
//! and free, amber is a cloud API and costing money, oxide red is stopped or
//! failed. Nothing else is ever coloured. Emphasis comes from weight and from
//! `TEXT` against `TEXT_DIM`, never from a fourth hue.

use ratatui::style::Color;

pub const BASE: Color = Color::Rgb(0x26, 0x25, 0x23);
pub const SURFACE: Color = Color::Rgb(0x30, 0x2E, 0x2B);
pub const LINE: Color = Color::Rgb(0x45, 0x43, 0x40);
pub const TEXT: Color = Color::Rgb(0xE4, 0xE1, 0xDB);
pub const TEXT_DIM: Color = Color::Rgb(0x8F, 0x8B, 0x84);
pub const TEXT_FAINT: Color = Color::Rgb(0x66, 0x62, 0x5C);

/// Running on the local model. Free. Private.
pub const LOCAL: Color = Color::Rgb(0x7A, 0x9B, 0x6E);
/// Cloud API in use. Costing money.
pub const API: Color = Color::Rgb(0xC8, 0x93, 0x4A);
/// Blocked, failed, needs you, over budget.
pub const STOP: Color = Color::Rgb(0xA8, 0x54, 0x45);

/// The tier colour for a provider. The only place this decision is made.
pub fn tier(local: bool) -> Color {
    if local {
        LOCAL
    } else {
        API
    }
}
