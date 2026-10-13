//! `utils/section_type.h`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum SectionType {
    NA = -1,
    WALL = 1,
    INFILL = 2,
    SKIN = 3,
    SUPPORT = 4,
    ADHESION = 5,
    IRONING = 6,
    MESH = 7,
    DOTS = 8,
    CONCENTRIC_INFILL = 9,
}
