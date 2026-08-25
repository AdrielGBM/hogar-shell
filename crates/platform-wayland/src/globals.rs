//! Asking the compositor what it advertises, from anywhere.
//!
//! The other "is this supported" answers in this crate read state the driver put there — `lock_supported` asks the driver facts, `idle_supported` a thread-local the event loop owns — which is right inside a running shell and silently wrong outside one, where both answer `false` because there is no driver rather than because the compositor is missing the protocol.
//!
//! That distinction is the whole point of asking: a dependency check runs in a bare CLI process, on the machine where something is broken, quite possibly *because* the shell will not start. So this connects on its own and reads the registry, and separates "the compositor does not have it" from "nothing here could tell".

use wayland_client::{Connection, globals::registry_queue_init};

struct Probe;

wayland_client::delegate_noop!(Probe: ignore wayland_client::protocol::wl_registry::WlRegistry);

impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_registry::WlRegistry,
        wayland_client::globals::GlobalListContents,
    > for Probe
{
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_registry::WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

/// Whether the compositor advertises `interface`, or `None` when this process cannot reach a compositor at all — no `WAYLAND_DISPLAY`, or a socket that will not answer.
///
/// `None` is not a failure to report: on a machine with no session running it is the *correct* answer, and a caller that flattened it to `false` would tell the user their compositor lacks a protocol it may implement perfectly well.
pub fn advertises(interface: &str) -> Option<bool> {
    advertises_all(&[interface])
}

/// Whether the compositor advertises *every* interface in `interfaces`, over one connection.
///
/// One protocol is not always one global: `ext-image-copy-capture` is a capture manager plus the factory that makes the sources it takes, and a compositor carrying one without the other can capture nothing. Asking for the set together is also what keeps this cheap enough to call from a surface deciding whether to offer a gesture — a connection and a registry read per interface would be a round trip per name.
pub fn advertises_all(interfaces: &[&str]) -> Option<bool> {
    let connection = Connection::connect_to_env().ok()?;
    let (globals, _queue) = registry_queue_init::<Probe>(&connection).ok()?;
    Some(globals.contents().with_list(|list| {
        let announced: Vec<&str> = list
            .iter()
            .map(|global| global.interface.as_str())
            .collect();
        all_present(&announced, interfaces)
    }))
}

fn all_present(announced: &[&str], wanted: &[&str]) -> bool {
    wanted.iter().all(|interface| announced.contains(interface))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Half a protocol is none of it — the rule the multi-interface rows in the dependency registry rest on.
    #[test]
    fn every_named_interface_has_to_be_there() {
        let announced = ["wl_shm", "ext_image_copy_capture_manager_v1"];
        assert!(all_present(&announced, &["wl_shm"]));
        assert!(!all_present(
            &announced,
            &[
                "ext_image_copy_capture_manager_v1",
                "ext_output_image_capture_source_manager_v1"
            ]
        ));
        assert!(
            all_present(&announced, &[]),
            "nothing wanted is nothing missing"
        );
    }
}
