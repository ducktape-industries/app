//! The app's state and its reducer.

mod app;
mod connect;
mod desk;
pub(crate) mod layout;
mod notify;
mod overlay;
mod panes;
mod sign_in;
mod update;

#[cfg(test)]
mod connect_tests;
#[cfg(test)]
mod desk_tests;
#[cfg(test)]
mod onboarding_tests;
#[cfg(test)]
mod sign_in_tests;
#[cfg(test)]
mod test_support;

pub(crate) use app::*;
