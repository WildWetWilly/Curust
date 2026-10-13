//! `PrintFeature.h`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum PrintFeatureType {
    NoneType = 0,
    OuterWall = 1,
    InnerWall = 2,
    Skin = 3,
    Support = 4,
    SkirtBrim = 5,
    Infill = 6,
    SupportInfill = 7,
    MoveUnretracted = 8,
    MoveRetracted = 9,
    SupportInterface = 10,
    PrimeTower = 11,
    MoveWhileRetracting = 12,
    MoveWhileUnretracting = 13,
    StationaryRetractUnretract = 14,
    NumPrintFeatureTypes = 15,
}
