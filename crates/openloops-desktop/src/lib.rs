#![deny(unsafe_code)]

#[cfg(feature = "native-ui")]
pub(crate) mod app_model;
#[cfg(feature = "native-ui")]
pub(crate) mod deadline_view;
#[cfg(feature = "native-ui")]
pub(crate) mod loop_state;
#[cfg(feature = "native-ui")]
pub(crate) mod review_model;
#[cfg(feature = "native-ui")]
pub(crate) mod settings;
#[cfg(feature = "native-ui")]
pub(crate) mod slint_review;
#[cfg(feature = "native-ui")]
pub mod slint_ui;
