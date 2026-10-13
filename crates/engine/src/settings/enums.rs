//! `settings/EnumSettings.h` and the string conversions from `Settings.cpp`.

/// Conversion of a setting string into an enum (`Settings::get<EnumType>`).
pub trait SettingEnum: Sized + Copy {
    fn from_setting(value: &str) -> Self;
}

macro_rules! setting_enum {
    ($name:ident { $($variant:ident),* $(,)? } default $default:ident; $($s:literal => $v:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),* }
        impl SettingEnum for $name {
            fn from_setting(value: &str) -> Self {
                match value {
                    $($s => $name::$v,)*
                    _ => $name::$default,
                }
            }
        }
    };
}

setting_enum!(EFillMethod {
    LINES, GRID, CUBIC, CUBICSUBDIV, TETRAHEDRAL, QUARTER_CUBIC, TRIANGLES, TRIHEXAGON, CONCENTRIC,
    ZIG_ZAG, CROSS, CROSS_3D, GYROID, LIGHTNING, HONEYCOMB, OCTAGON, NONE, PLUGIN,
} default NONE;
    "none" => NONE, "lines" => LINES, "grid" => GRID, "cubic" => CUBIC, "cubicsubdiv" => CUBICSUBDIV,
    "tetrahedral" => TETRAHEDRAL, "quarter_cubic" => QUARTER_CUBIC, "triangles" => TRIANGLES,
    "trihexagon" => TRIHEXAGON, "concentric" => CONCENTRIC, "zigzag" => ZIG_ZAG, "cross" => CROSS,
    "cross_3d" => CROSS_3D, "gyroid" => GYROID, "lightning" => LIGHTNING, "honeycomb" => HONEYCOMB,
    "octagon" => OCTAGON, "plugin" => PLUGIN,
);

setting_enum!(EPlatformAdhesion { SKIRT, BRIM, RAFT, NONE, PLUGIN } default SKIRT;
    "skirt" => SKIRT, "brim" => BRIM, "raft" => RAFT, "none" => NONE, "plugin" => PLUGIN,
);

setting_enum!(ESupportType { NONE, PLATFORM_ONLY, EVERYWHERE, PLUGIN } default NONE;
    "none" => NONE, "everywhere" => EVERYWHERE, "buildplate" => PLATFORM_ONLY, "plugin" => PLUGIN,
);

setting_enum!(ESupportStructure { NORMAL, TREE, PLUGIN } default NORMAL;
    "normal" => NORMAL, "tree" => TREE, "plugin" => PLUGIN,
);

setting_enum!(EZSeamType { RANDOM, SHORTEST, USER_SPECIFIED, SHARPEST_CORNER, SKIRT_BRIM, PLUGIN } default SHORTEST;
    "shortest" => SHORTEST, "random" => RANDOM, "back" => USER_SPECIFIED, "sharpest_corner" => SHARPEST_CORNER,
    "plugin" => PLUGIN,
);

setting_enum!(EZSeamCornerPrefType {
    Z_SEAM_CORNER_PREF_INNER, Z_SEAM_CORNER_PREF_OUTER, Z_SEAM_CORNER_PREF_ANY, Z_SEAM_CORNER_PREF_WEIGHTED, PLUGIN,
} default Z_SEAM_CORNER_PREF_INNER;
    "z_seam_corner_outer" => Z_SEAM_CORNER_PREF_OUTER, "z_seam_corner_any" => Z_SEAM_CORNER_PREF_ANY,
    "z_seam_corner_weighted" => Z_SEAM_CORNER_PREF_WEIGHTED, "plugin" => PLUGIN,
    "z_seam_corner_inner" => Z_SEAM_CORNER_PREF_INNER,
);

setting_enum!(ESurfaceMode { NORMAL, SURFACE, BOTH, PLUGIN } default NORMAL;
    "normal" => NORMAL, "surface" => SURFACE, "both" => BOTH, "plugin" => PLUGIN,
);

setting_enum!(FillPerimeterGapMode { NOWHERE, EVERYWHERE, PLUGIN } default NOWHERE;
    "nowhere" => NOWHERE, "everywhere" => EVERYWHERE, "plugin" => PLUGIN,
);

setting_enum!(BuildPlateShape { RECTANGULAR, ELLIPTIC, PLUGIN } default RECTANGULAR;
    "rectangular" => RECTANGULAR, "elliptic" => ELLIPTIC, "plugin" => PLUGIN,
);

setting_enum!(CombingMode { OFF, ALL, NO_SKIN, NO_OUTER_SURFACES, INFILL, PLUGIN } default ALL;
    "all" => ALL, "off" => OFF, "noskin" => NO_SKIN, "no_outer_surfaces" => NO_OUTER_SURFACES,
    "infill" => INFILL, "plugin" => PLUGIN,
);

setting_enum!(DraftShieldHeightLimitation { FULL, LIMITED, PLUGIN } default FULL;
    "full" => FULL, "limited" => LIMITED, "plugin" => PLUGIN,
);

setting_enum!(SupportDistPriority { XY_OVERRIDES_Z, Z_OVERRIDES_XY, PLUGIN } default XY_OVERRIDES_Z;
    "xy_overrides_z" => XY_OVERRIDES_Z, "z_overrides_xy" => Z_OVERRIDES_XY, "plugin" => PLUGIN,
);

setting_enum!(SlicingTolerance { MIDDLE, INCLUSIVE, EXCLUSIVE, PLUGIN } default MIDDLE;
    "middle" => MIDDLE, "inclusive" => INCLUSIVE, "exclusive" => EXCLUSIVE, "plugin" => PLUGIN,
);

setting_enum!(InsetDirection { INSIDE_OUT, OUTSIDE_IN, CENTER_LAST, PLUGIN } default INSIDE_OUT;
    "inside_out" => INSIDE_OUT, "outside_in" => OUTSIDE_IN, "plugin" => PLUGIN,
);

setting_enum!(PrimeTowerMode { INTERLEAVED, NORMAL } default NORMAL;
    "interleaved" => INTERLEAVED,
);

setting_enum!(CoolDuringExtruderSwitch { UNCHANGED, ONLY_LAST_EXTRUDER, ALL_FANS } default UNCHANGED;
    "all_fans" => ALL_FANS, "only_last_extruder" => ONLY_LAST_EXTRUDER,
);

/// G-code flavours. The discriminants match the C++ values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EGCodeFlavor {
    MARLIN = 0,
    ULTIGCODE = 1,
    MAKERBOT = 2,
    BFB = 3,
    MACH3 = 4,
    MARLIN_VOLUMATRIC = 5,
    GRIFFIN = 6,
    CHEETAH = 61,
    REPETIER = 7,
    REPRAP = 8,
    PLUGIN = 9,
}

impl SettingEnum for EGCodeFlavor {
    fn from_setting(value: &str) -> Self {
        match value {
            "Marlin" => EGCodeFlavor::MARLIN,
            "Griffin" => EGCodeFlavor::GRIFFIN,
            "Cheetah" => EGCodeFlavor::CHEETAH,
            "UltiGCode" => EGCodeFlavor::ULTIGCODE,
            "Makerbot" => EGCodeFlavor::MAKERBOT,
            "BFB" => EGCodeFlavor::BFB,
            "MACH3" => EGCodeFlavor::MACH3,
            "RepRap (Volumetric)" => EGCodeFlavor::MARLIN_VOLUMATRIC,
            "Repetier" => EGCodeFlavor::REPETIER,
            "RepRap (RepRap)" => EGCodeFlavor::REPRAP,
            "plugin" => EGCodeFlavor::PLUGIN,
            _ => EGCodeFlavor::MARLIN,
        }
    }
}

/// Brim location bit flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BrimLocation {
    OUTSIDE = 0x01,
    INSIDE = 0x02,
    EVERYWHERE = 0x03,
}

impl BrimLocation {
    /// `operator&(BrimLocation, BrimLocation)`.
    pub fn and(self, other: BrimLocation) -> i32 {
        (self as i32) & (other as i32)
    }
}

impl SettingEnum for BrimLocation {
    fn from_setting(value: &str) -> Self {
        match value {
            "everywhere" => BrimLocation::EVERYWHERE,
            "inside" => BrimLocation::INSIDE,
            _ => BrimLocation::OUTSIDE,
        }
    }
}
