use serde_json::Value;

const FOREGROUND_URI: &str = "luna://com.webos.applicationManager/getForegroundAppInfo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSource {
    Hdmi,
    Unsupported,
    Unknown,
}

impl CaptureSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hdmi => "HDMI",
            Self::Unsupported => "Unsupported",
            Self::Unknown => "Unknown",
        }
    }

    pub fn compatible(self) -> Option<bool> {
        match self {
            Self::Hdmi => Some(true),
            Self::Unsupported => Some(false),
            Self::Unknown => None,
        }
    }
}

/// Policy input for a foreground observation, given whether the active capture
/// backend supports non-HDMI apps. The aggregate VtHistogram backend does —
/// it reads only per-zone color statistics from the video hardware, so
/// built-in apps and Home are observable, and reporting them as incompatible
/// would block DRM-protected playback and defeat the backend's purpose.
/// Frame-capture backends keep the original policy: `Unsupported` stays
/// incompatible, `Unknown` preserves the last known policy.
pub fn compatible_for(source: CaptureSource, app_sources: bool) -> Option<bool> {
    match source {
        CaptureSource::Unsupported if app_sources => None,
        _ => source.compatible(),
    }
}

pub(crate) fn parse_source(value: &Value) -> CaptureSource {
    if value["returnValue"] != true {
        return CaptureSource::Unknown;
    }
    let Some(id) = value["appId"].as_str().filter(|id| {
        !id.is_empty()
            && id.len() <= 256
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    }) else {
        return CaptureSource::Unknown;
    };
    match id {
        "com.webos.app.hdmi1"
        | "com.webos.app.hdmi2"
        | "com.webos.app.hdmi3"
        | "com.webos.app.hdmi4" => CaptureSource::Hdmi,
        _ => CaptureSource::Unsupported,
    }
}

/// Foreground identity is a capture-policy hint, not DRM detection.
pub async fn current_source() -> CaptureSource {
    crate::luna::query(FOREGROUND_URI, "{}")
        .await
        .as_ref()
        .map(parse_source)
        .unwrap_or(CaptureSource::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn app_source_support_turns_unsupported_into_unknown_and_keeps_the_rest() {
        for source in [CaptureSource::Hdmi, CaptureSource::Unknown] {
            assert_eq!(
                crate::capture_source::compatible_for(source, true),
                source.compatible()
            );
        }
        for source in [CaptureSource::Hdmi, CaptureSource::Unknown] {
            assert_eq!(
                crate::capture_source::compatible_for(source, false),
                source.compatible()
            );
        }
        // Frame backends keep the original policy for non-HDMI foreground apps.
        assert_eq!(
            crate::capture_source::compatible_for(CaptureSource::Unsupported, false),
            Some(false)
        );
        // Aggregate backends can observe them, so the policy must not block.
        assert_eq!(
            crate::capture_source::compatible_for(CaptureSource::Unsupported, true),
            None
        );
    }

    #[test]
    fn foreground_identity_classifies_only_exact_hdmi_ids_and_preserves_unknowns() {
        for id in [
            "com.webos.app.hdmi1",
            "com.webos.app.hdmi2",
            "com.webos.app.hdmi3",
            "com.webos.app.hdmi4",
        ] {
            let source =
                parse_source(&json!({"returnValue":true,"appId":id,"title":"private title"}));
            assert_eq!(source, CaptureSource::Hdmi);
            assert_eq!(source.compatible(), Some(true));
            assert_eq!(source.label(), "HDMI");
        }
        for id in [
            "netflix",
            "com.webos.app.netflix",
            "com.webos.app.home",
            "com.webos.app.browser",
            "com.webos.app.livetv",
            "com.webos.app.hdmi5",
            "com.webos.app.hdmi1.extra",
        ] {
            let source = parse_source(&json!({"returnValue":true,"appId":id}));
            assert_eq!(source, CaptureSource::Unsupported);
            assert_eq!(source.compatible(), Some(false));
        }
        for value in [
            json!({}),
            json!({"appId":"com.webos.app.hdmi1"}),
            json!({"returnValue":false,"appId":"netflix"}),
            json!({"returnValue":true,"appId":""}),
            json!({"returnValue":true,"appId":12}),
            json!({"returnValue":true,"appId":"bad\napp"}),
            json!({"returnValue":true,"foregroundAppInfo":[{"appId":"netflix"}]}),
        ] {
            assert_eq!(parse_source(&value), CaptureSource::Unknown);
        }
        assert_eq!(
            parse_source(&json!("not an object")),
            CaptureSource::Unknown
        );
        assert_eq!(CaptureSource::Unknown.compatible(), None);
        assert_eq!(CaptureSource::Unknown.label(), "Unknown");
    }
}
