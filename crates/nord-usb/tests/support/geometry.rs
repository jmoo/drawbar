//! The committed recording of `device geometry`, replayed on a transport of its own.
//!
//! An Electro 5 script without a geometry section is bounded by these tables, which are
//! static configuration. A new recording carries its own.
//!
//! ⚠️ A support module, not a test target: each test target that includes it
//! compiles its own copy. It reads the crate-root `scripts` module.
#![allow(dead_code)]

use nord_usb::device::Geometry;
use nord_usb::transport::ReplayTransport;
use nord_usb::wire::ObjectClass;
use nord_usb::{Result, Session};

pub async fn committed() -> Result<Geometry> {
    let mut t = ReplayTransport::new(crate::scripts::fixture("device/geometry.script").steps());
    let mut session = Session::open(&mut t, ObjectClass::Program).await?;
    let read = Geometry::read(&mut session).await;
    let closed = session.commit().await;
    read.and_then(|geometry| closed.map(|()| geometry))
}
