//! What no fixture can prove: that the services speak to the daemons of a real session. Opt-in, since each test opens the process-wide session gate the unit tests rely on staying shut.

use std::collections::HashMap;
use std::time::Duration;

use zbus::blocking::fdo::PropertiesProxy;
use zbus::zvariant::Value;

use hogar_shell_services as services;
use services::notifications::{self, Policy};

/// Reads the battery's level off UPower. `HOGAR_SHELL_TEST_UPOWER=1 cargo test -p hogar-shell-services --test live_session upower -- --nocapture`
#[test]
fn upower_connection_reads_percentage() {
    if std::env::var("HOGAR_SHELL_TEST_UPOWER").is_err() {
        eprintln!("set HOGAR_SHELL_TEST_UPOWER to read the real UPower; skipping");
        return;
    }
    util::live::install();
    let conn = services::bus::system(None).expect("system bus");
    let props = PropertiesProxy::builder(&conn)
        .destination("org.freedesktop.UPower")
        .unwrap()
        .path("/org/freedesktop/UPower/devices/DisplayDevice")
        .unwrap()
        .build()
        .expect("build DisplayDevice proxy");
    let pct = props
        .get(
            "org.freedesktop.UPower.Device".try_into().unwrap(),
            "Percentage",
        )
        .expect("read Percentage");
    eprintln!("UPower DisplayDevice Percentage = {pct:?}");
}

/// A notification sent over D-Bus reaches the daemon. Under a private bus, so it never collides with the desktop's own daemon: `dbus-run-session -- cargo test -p hogar-shell-services --test live_session daemon -- --ignored --nocapture`
#[test]
#[ignore = "needs a session bus; run under dbus-run-session"]
fn daemon_receives_notify_over_dbus() {
    util::live::install();
    notifications::init(Policy {
        timeout: Duration::from_millis(5000),
        critical_sticky: true,
        critical_max: Some(Duration::from_secs(120)),
        sound: String::new(),
    });
    let client = services::bus::private_session(None).expect("session bus");
    let hints: HashMap<&str, Value> = HashMap::new();
    let mut sent = false;
    for _ in 0..50 {
        let call = client.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "test-app",
                0u32,
                "",
                "Hello",
                "World",
                Vec::<&str>::new(),
                &hints,
                -1i32,
            ),
        );
        if call.is_ok() {
            sent = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(sent, "daemon claimed the name and answered Notify");

    let snapshot = notifications::snapshot_now().expect("service initialized");
    assert_eq!(snapshot.active.len(), 1);
    assert_eq!(snapshot.active[0].summary, "Hello");
    assert_eq!(snapshot.active[0].body, "World");
    assert_eq!(snapshot.unread, 1);
}

/// Reads a real application's menu off the session bus: `HOGAR_SHELL_TEST_DBUSMENU=<bus><path> cargo test -p hogar-shell-services --test live_session dbusmenu -- --nocapture`, e.g. `HOGAR_SHELL_TEST_DBUSMENU=":1.502/org/ayatana/NotificationItem/steam/Menu"`.
#[test]
fn dbusmenu_reads_a_live_menu() {
    let Ok(target) = std::env::var("HOGAR_SHELL_TEST_DBUSMENU") else {
        eprintln!("set HOGAR_SHELL_TEST_DBUSMENU to read a live menu; skipping");
        return;
    };
    util::live::install();
    let (bus, path) = target.split_once('/').expect("bus/path");
    let root = services::dbusmenu::fetch(bus, &format!("/{path}"))
        .expect("the application answered GetLayout");
    for child in &root.children {
        eprintln!(
            "  [{}] {:?} enabled={} separator={} submenu={} toggle={:?}",
            child.id,
            child.label,
            child.enabled,
            child.separator,
            child.has_submenu(),
            child.toggle
        );
    }
    assert!(!root.children.is_empty(), "a real menu has rows");
}
