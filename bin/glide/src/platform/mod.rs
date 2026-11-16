//! Platform layer. Only Windows is implemented in v1; the engine never sees
//! these types, so other back-ends (macOS `CGEventTap`, Linux `uinput`) can be
//! added later behind the same `animator::Backend` seam.

pub mod win32;
