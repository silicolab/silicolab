use anyhow::{Context, Result};
use eframe::egui::Color32;
use rusqlite::{Connection, params};

use crate::frontend::{LightPreset, SurfaceStyle};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderScope {
    pub(crate) scope_type: &'static str,
    pub(crate) scope_id: ScopeId,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ScopeId {
    Project,
    Entry(u64),
}

impl RenderScope {
    pub(crate) fn project() -> Self {
        Self {
            scope_type: "project",
            scope_id: ScopeId::Project,
        }
    }

    pub(crate) fn entry(entry_id: u64) -> Self {
        Self {
            scope_type: "entry",
            scope_id: ScopeId::Entry(entry_id),
        }
    }
}

impl std::fmt::Display for ScopeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project => formatter.write_str("project"),
            Self::Entry(entry_id) => write!(formatter, "{entry_id}"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RenderTarget<'a> {
    scope_type: &'a str,
    scope_id: String,
    target_type: &'a str,
    target_id: String,
    priority: i64,
}

impl<'a> RenderTarget<'a> {
    pub(crate) fn view(scope: RenderScope) -> Self {
        Self {
            scope_type: scope.scope_type,
            scope_id: scope.scope_id.to_string(),
            target_type: "view",
            target_id: "default".to_string(),
            priority: 0,
        }
    }

    pub(crate) fn chain(scope: RenderScope, chain: char) -> Self {
        Self {
            scope_type: scope.scope_type,
            scope_id: scope.scope_id.to_string(),
            target_type: "chain",
            target_id: char_to_string(chain),
            priority: 20,
        }
    }

    pub(crate) fn atom_category(scope: RenderScope, category: &'a str) -> Self {
        Self {
            scope_type: scope.scope_type,
            scope_id: scope.scope_id.to_string(),
            target_type: "atom_category",
            target_id: category.to_string(),
            priority: 10,
        }
    }

    pub(crate) fn atom(scope: RenderScope, atom_index: usize) -> Self {
        Self {
            scope_type: scope.scope_type,
            scope_id: scope.scope_id.to_string(),
            target_type: "atom",
            target_id: atom_index.to_string(),
            priority: 30,
        }
    }
}

pub(crate) struct RenderOverrideRow {
    pub(crate) scope_type: String,
    pub(crate) scope_id: String,
    pub(crate) target_type: String,
    pub(crate) target_id: String,
    pub(crate) property: String,
    pub(crate) value_type: String,
    pub(crate) value_text: Option<String>,
    pub(crate) value_real: Option<f64>,
    pub(crate) value_integer: Option<i64>,
    pub(crate) value_json: Option<String>,
}

impl RenderOverrideRow {
    pub(crate) fn json_value(&self) -> Result<Option<serde_json::Value>> {
        let Some(source) = self.value_json.as_deref() else {
            return Ok(None);
        };
        serde_json::from_str(source)
            .with_context(|| format!("failed to parse render override {}", self.property))
            .map(Some)
    }
}

struct RenderOverrideValue<'a> {
    value_type: &'a str,
    value_text: Option<&'a str>,
    value_real: Option<f64>,
    value_integer: Option<i64>,
    value_json: Option<&'a str>,
}

pub(crate) fn set_render_override_text(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: &str,
) -> Result<()> {
    insert_render_override(
        db,
        target,
        property,
        RenderOverrideValue {
            value_type: "text",
            value_text: Some(value),
            value_real: None,
            value_integer: None,
            value_json: None,
        },
    )
}

pub(crate) fn set_render_override_real(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: f32,
) -> Result<()> {
    insert_render_override(
        db,
        target,
        property,
        RenderOverrideValue {
            value_type: "real",
            value_text: None,
            value_real: Some(f64::from(value)),
            value_integer: None,
            value_json: None,
        },
    )
}

pub(crate) fn set_render_override_integer(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: i64,
) -> Result<()> {
    insert_render_override(
        db,
        target,
        property,
        RenderOverrideValue {
            value_type: "integer",
            value_text: None,
            value_real: None,
            value_integer: Some(value),
            value_json: None,
        },
    )
}

pub(crate) fn set_render_override_bool(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: bool,
) -> Result<()> {
    set_render_override_integer(db, target, property, bool_to_i64(value))
}

pub(crate) fn set_render_override_json(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: serde_json::Value,
) -> Result<()> {
    insert_render_override(
        db,
        target,
        property,
        RenderOverrideValue {
            value_type: "json",
            value_text: None,
            value_real: None,
            value_integer: None,
            value_json: Some(&value.to_string()),
        },
    )
}

fn insert_render_override(
    db: &Connection,
    target: RenderTarget<'_>,
    property: &str,
    value: RenderOverrideValue<'_>,
) -> Result<()> {
    db.execute(
        "insert into render_overrides (scope_type, scope_id, target_type, target_id, property, value_type, value_text, value_real, value_integer, value_json, priority) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            target.scope_type,
            target.scope_id,
            target.target_type,
            target.target_id,
            property,
            value.value_type,
            value.value_text,
            value.value_real,
            value.value_integer,
            value.value_json,
            target.priority,
        ],
    )?;
    Ok(())
}

pub(crate) fn light_preset_token(preset: LightPreset) -> &'static str {
    match preset {
        LightPreset::Soft => "soft",
        LightPreset::Gentle => "gentle",
        LightPreset::Studio => "studio",
    }
}

pub(crate) fn parse_light_preset(token: &str) -> LightPreset {
    match token {
        "gentle" => LightPreset::Gentle,
        "studio" => LightPreset::Studio,
        _ => LightPreset::Soft,
    }
}

pub(crate) fn surface_style_token(style: SurfaceStyle) -> &'static str {
    match style {
        SurfaceStyle::Fill => "fill",
        SurfaceStyle::Mesh => "mesh",
    }
}

pub(crate) fn parse_surface_style(token: &str) -> SurfaceStyle {
    match token {
        "mesh" => SurfaceStyle::Mesh,
        _ => SurfaceStyle::Fill,
    }
}

pub(crate) fn color_json(color: Color32) -> serde_json::Value {
    serde_json::json!([color.r(), color.g(), color.b(), color.a()])
}

pub(crate) fn parse_color_json(value: &serde_json::Value) -> Option<Color32> {
    let channels = value.as_array()?;
    Some(Color32::from_rgba_unmultiplied(
        channels.first()?.as_u64()? as u8,
        channels.get(1)?.as_u64()? as u8,
        channels.get(2)?.as_u64()? as u8,
        channels.get(3)?.as_u64()? as u8,
    ))
}

pub(crate) fn set_bool_from_integer(value: Option<i64>, target: &mut bool) {
    if let Some(value) = value {
        *target = value != 0;
    }
}

pub(crate) fn set_f32_from_real(value: Option<f64>, target: &mut f32) {
    if let Some(value) = value {
        *target = value as f32;
    }
}

pub(crate) fn string_to_char(value: &str) -> char {
    value.chars().next().unwrap_or(' ')
}

fn char_to_string(value: char) -> String {
    value.to_string()
}

fn bool_to_i64(value: bool) -> i64 {
    i64::from(value)
}
