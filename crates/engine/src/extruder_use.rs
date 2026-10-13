//! `ExtruderUse.h`

/// How an extruder is used on a layer, regarding the prime tower.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExtruderPrime {
    /// Do not prime at all for this extruder on this layer
    #[default]
    None,
    /// Just extrude a sparse pattern which purpose is to support the upper parts of the prime tower
    Support,
    /// Do an actual prime
    Prime,
}

/// `ExtruderUse`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtruderUse {
    pub extruder_nr: usize,
    pub prime: ExtruderPrime,
}
