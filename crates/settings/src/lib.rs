//! Curust settings system: a port of the Uranium/Cura 5.12 settings machinery (setting
//! definitions, profiles, container stacks and formula evaluation).

pub mod py;
pub mod definition;
pub mod instance;
pub mod json;
pub mod material;
pub mod registry;
pub mod xml;
pub mod stack;
pub mod machine;
pub mod slice_message;
pub mod upgrade;
