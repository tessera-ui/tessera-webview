use super::*;
use crate::{IpcPolicy, NavigationPolicy, WebViewController};

fn rect(x: f32, y: f32, width: f32, height: f32) -> FloatRect {
    FloatRect {
        x,
        y,
        width,
        height,
    }
}

fn configured_plugin() -> WebViewPlugin {
    let mut plugin = WebViewPlugin::new();
    plugin
        .configure(
            1,
            10,
            WebViewSource::html("one"),
            WebViewOptions::default(),
            None,
        )
        .unwrap();
    plugin
}

#[test]
fn native_surface_hides_when_unplaced_partly_clipped_or_nonfinite() {
    let full = rect(20.25, 30.5, 100.5, 80.25);
    let mut geometry = Geometry {
        bounds: Some(full),
        visible: Some(full),
    };
    assert!(geometry.is_visible());
    geometry.visible = Some(rect(20.25, 30.5, f32::INFINITY, 80.25));
    assert!(!geometry.is_visible());
    geometry.visible = Some(rect(21.0, 30.5, 99.75, 80.25));
    assert!(!geometry.is_visible());
    geometry.visible = None;
    assert!(!geometry.is_visible());
    geometry.visible = Some(full);
    geometry.bounds = Some(rect(f32::NAN, 0.0, 10.0, 10.0));
    assert!(!geometry.is_visible());
    geometry.bounds = Some(rect(0.0, 0.0, 0.0, 10.0));
    assert!(!geometry.is_visible());
}

#[test]
fn controller_cannot_be_stolen_or_detached_by_a_second_component() {
    let mut plugin = configured_plugin();
    assert_eq!(
        plugin.configure(
            1,
            20,
            WebViewSource::html("two"),
            WebViewOptions::default(),
            None
        ),
        Err(WebViewError::ControllerInUse)
    );
    plugin.detach(1, 20);
    assert!(matches!(
        plugin.native(1),
        Err(WebViewError::HostNotResumed)
    ));
    plugin.detach(1, 10);
    assert!(matches!(plugin.native(1), Err(WebViewError::NotMounted)));
}

#[test]
fn evaluation_requires_a_resumed_host() {
    let mut plugin = configured_plugin();
    assert_eq!(
        plugin.evaluate(1, "42".into()).unwrap_err(),
        WebViewError::HostNotResumed
    );
}

#[test]
fn releasing_a_native_generation_cancels_evaluation_and_ignores_old_results() {
    let mut plugin = configured_plugin();
    let (sender, mut response) = oneshot::channel();
    plugin
        .entries
        .get_mut(&1)
        .unwrap()
        .evaluations
        .insert(3, sender);
    let old = plugin.entries[&1].generation;
    plugin.entries.get_mut(&1).unwrap().release();
    assert!(response.try_recv().is_err());
    let (sender, mut response) = oneshot::channel();
    plugin
        .entries
        .get_mut(&1)
        .unwrap()
        .evaluations
        .insert(3, sender);
    plugin.dispatch(Envelope {
        id: 1,
        generation: old,
        payload: Payload::Script {
            request: 3,
            value: "stale".into(),
        },
    });
    assert_eq!(response.try_recv(), Ok(None));
    plugin.dispatch(Envelope {
        id: 1,
        generation: plugin.entries[&1].generation,
        payload: Payload::Script {
            request: 3,
            value: "42".into(),
        },
    });
    assert_eq!(response.try_recv(), Ok(Some("42".into())));
}

#[test]
fn navigation_and_ipc_compare_parsed_origins_not_prefixes() {
    let navigation = NavigationPolicy::Origins(vec!["https://EXAMPLE.com:443".into()]);
    let ipc = IpcPolicy::Origins(vec!["https://example.com".into()]);
    for value in ["https://example.com/help", "https://example.com:443/help"] {
        assert!(navigation.allows(value));
        assert!(ipc.allows(value));
    }
    for value in [
        "https://example.com.evil.test",
        "https://example.com:444",
        "http://example.com",
        "https://example.com@evil.test",
        "https://user@example.com",
        "file:///tmp/secret",
        "javascript:alert(1)",
        "about:blank",
        "null",
    ] {
        assert!(!ipc.allows(value), "{value}");
        if value != "about:blank" {
            assert!(!navigation.allows(value), "{value}");
        }
    }
    assert!(navigation.allows("about:blank"));
    assert!(!IpcPolicy::default().allows("https://example.com"));
}

#[test]
fn invalid_origin_configuration_and_dangerous_initial_urls_are_rejected() {
    for origin in [
        "https://example.com/path",
        "https://example.com?query",
        "https://user@example.com",
        "file:///",
        "not a URL",
    ] {
        let options = WebViewOptions {
            ipc: IpcPolicy::Origins(vec![origin.into()]),
            ..Default::default()
        };
        assert!(
            matches!(
                options.validate(),
                Err(WebViewError::InvalidConfiguration(_))
            ),
            "{origin}"
        );
    }
    for url in [
        "javascript:alert(1)",
        "file:///C:/secret",
        "data:text/html,hi",
        "https://user:password@example.com",
    ] {
        assert!(matches!(
            WebViewSource::url(url).validate(&NavigationPolicy::Web),
            Err(WebViewError::NavigationDenied(_))
        ));
    }
}

#[test]
fn invalid_zoom_is_reported_before_accessing_the_native_engine() {
    let controller = WebViewController::new();
    for factor in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
        assert!(matches!(
            controller.set_zoom(factor),
            Err(WebViewError::InvalidConfiguration(_))
        ));
    }
}
