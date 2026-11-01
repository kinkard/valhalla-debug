//! Valhalla requests from the page's spec, parsed by Valhalla's JSON parser and sent as protobuf:
//! only protobuf honors `pbf_field_selector`.

use serde::Deserialize;
use serde_json::{Map, Value, json};
use valhalla::{Actor, proto};

/// The page's request state, see `web/js/request.js`.
#[derive(Deserialize, Debug, Default)]
pub struct Spec {
    pub costing: String,
    #[serde(default)]
    pub locations: Vec<Map<String, Value>>,
    #[serde(default)]
    pub opts: Map<String, Value>,
}

/// The request parsed by Valhalla, and its parsing warnings, e.g. for out-of-range options: Valhalla
/// drops them from protobuf requests.
pub fn options(
    spec: &Spec,
    action: proto::options::Action,
) -> Result<(proto::Options, Vec<proto::CodedDescription>), String> {
    let json = json(spec).to_string();
    let api = Actor::parse_json_api(&json, action).map_err(|err| err.to_string())?;
    let warnings = api.info.map(|info| info.warnings).unwrap_or_default();
    Ok((api.options.unwrap_or_default(), warnings))
}

/// Valhalla's JSON request: `co.` options go to `costing_options`, `date_time.` ones to `date_time`.
fn json(spec: &Spec) -> Value {
    let mut request = json!({ "costing": spec.costing, "locations": spec.locations });
    for (name, v) in &spec.opts {
        let field = match (name.strip_prefix("co."), name.strip_prefix("date_time.")) {
            (Some(name), _) => &mut request["costing_options"][&spec.costing][name],
            (_, Some(name)) => &mut request["date_time"][name],
            _ => &mut request[name],
        };
        *field = v.clone();
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::{costing, location, options};

    fn spec(opts: Value) -> Spec {
        let location = |l: Value| l.as_object().unwrap().clone();
        let via = json!({"lat": 55.65, "lon": 13.1, "type": "break_through", "street_side_cutoff": "service_other"});
        Spec {
            costing: "auto".into(),
            locations: vec![
                location(json!({"lat": 55.6, "lon": 13.0})),
                location(via),
                location(json!({"lat": 55.7, "lon": 13.2})),
            ],
            opts: opts.as_object().unwrap().clone(),
        }
    }

    fn route(spec: &Spec) -> Result<(proto::Options, Vec<proto::CodedDescription>), String> {
        options(spec, options::Action::Route)
    }

    #[test]
    fn maps_names_values_and_enums() {
        let (o, _) = route(&spec(json!({
            "units": "miles", "date_time.type": 1, "date_time.value": "2026-10-30T08:00",
            "reverse_time_tracking": "disabled", "co.use_highways": 0.25, "co.speed_types": ["freeflow", "predicted"],
            "co.name": "mine",
        })))
        .unwrap();
        assert_eq!(o.units, options::Units::Miles as i32);
        assert_eq!(o.date_time_type, options::DateTimeType::DepartAt as i32);
        assert_eq!(
            o.reverse_time_tracking,
            options::ReverseTimeTracking::RttDisabled as i32
        );
        let l = &o.locations[1];
        assert_eq!(l.r#type, location::Type::KBreakThrough as i32);
        assert_eq!(
            l.has_street_side_cutoff,
            Some(location::HasStreetSideCutoff::StreetSideCutoff(
                proto::RoadClass::KServiceOther as i32
            ))
        );
        let costing = &o.costings[&(costing::Type::Auto as i32)];
        assert_eq!(
            costing.has_name,
            Some(costing::HasName::Name("mine".into()))
        );
        let Some(costing::HasOptions::Options(co)) = &costing.has_options else {
            panic!("no costing options")
        };
        assert_eq!(
            co.has_use_highways,
            Some(costing::options::HasUseHighways::UseHighways(0.25))
        );
        assert_eq!(
            co.has_flow_mask,
            Some(costing::options::HasFlowMask::FlowMask(5))
        );
    }

    #[test]
    fn out_of_range_options_are_warnings() {
        let (_, warnings) = route(&spec(json!({"co.use_highways": 2}))).unwrap();
        assert!(warnings.iter().any(|w| w.code == 500), "{warnings:?}");
    }

    #[test]
    fn unknown_costing_is_an_error() {
        let bad_costing = Spec {
            costing: "hover".into(),
            ..spec(json!({}))
        };
        assert!(route(&bad_costing).is_err());
    }
}
