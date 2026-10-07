//! Minimal JSON Schema validator for the subset our theme schemas use:
//! type, required, properties, additionalProperties(false), enum, pattern,
//! minLength/maxLength, minimum/maximum, items, minItems/maxItems, $ref to
//! `#/$defs/...`. Errors carry a JSON pointer to the bad value.

use serde_json::Value;

pub fn validate(schema: &Value, value: &Value) -> Result<(), Vec<String>> {
    let mut errs = Vec::new();
    check(schema, schema, value, "", &mut errs);
    if errs.is_empty() { Ok(()) } else { Err(errs) }
}

fn resolve<'a>(root: &'a Value, s: &'a Value) -> &'a Value {
    if let Some(r) = s.get("$ref").and_then(|r| r.as_str()) {
        if let Some(name) = r.strip_prefix("#/$defs/") {
            if let Some(d) = root.get("$defs").and_then(|d| d.get(name)) {
                return d;
            }
        }
    }
    s
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "number" => v.is_number(),
        "integer" => v.as_i64().is_some() || v.as_u64().is_some(),
        "null" => v.is_null(),
        _ => true,
    }
}

fn check(root: &Value, schema: &Value, v: &Value, path: &str, errs: &mut Vec<String>) {
    let s = resolve(root, schema);
    let at = if path.is_empty() { "/" } else { path };
    if let Some(t) = s.get("type").and_then(|t| t.as_str()) {
        if !type_ok(t, v) {
            errs.push(format!("{at}: expected {t}"));
            return;
        }
    }
    if let Some(e) = s.get("enum").and_then(|e| e.as_array()) {
        if !e.contains(v) {
            errs.push(format!("{at}: must be one of {}", Value::Array(e.clone())));
        }
    }
    if let Some(st) = v.as_str() {
        let n = st.chars().count() as u64;
        if let Some(m) = s.get("minLength").and_then(|m| m.as_u64()) {
            if n < m {
                errs.push(format!("{at}: shorter than {m}"));
            }
        }
        if let Some(m) = s.get("maxLength").and_then(|m| m.as_u64()) {
            if n > m {
                errs.push(format!("{at}: longer than {m}"));
            }
        }
        if let Some(p) = s.get("pattern").and_then(|p| p.as_str()) {
            match regex::Regex::new(p) {
                Ok(re) if !re.is_match(st) => errs.push(format!("{at}: invalid value {:?}", st.chars().take(40).collect::<String>())),
                Err(_) => errs.push(format!("{at}: bad schema pattern")),
                _ => {}
            }
        }
    }
    if let Some(x) = v.as_f64() {
        if let Some(m) = s.get("minimum").and_then(|m| m.as_f64()) {
            if x < m {
                errs.push(format!("{at}: below minimum {m}"));
            }
        }
        if let Some(m) = s.get("maximum").and_then(|m| m.as_f64()) {
            if x > m {
                errs.push(format!("{at}: above maximum {m}"));
            }
        }
    }
    if let Some(obj) = v.as_object() {
        let props = s.get("properties").and_then(|p| p.as_object());
        if let Some(req) = s.get("required").and_then(|r| r.as_array()) {
            for r in req.iter().filter_map(|r| r.as_str()) {
                if !obj.contains_key(r) {
                    errs.push(format!("{at}: missing required \"{r}\""));
                }
            }
        }
        for (k, val) in obj {
            let child = format!("{path}/{k}");
            match props.and_then(|p| p.get(k)) {
                Some(ps) => check(root, ps, val, &child, errs),
                None => {
                    if s.get("additionalProperties") == Some(&Value::Bool(false)) {
                        errs.push(format!("{at}: unknown property \"{k}\""));
                    }
                }
            }
        }
    }
    if let Some(arr) = v.as_array() {
        if let Some(m) = s.get("minItems").and_then(|m| m.as_u64()) {
            if (arr.len() as u64) < m {
                errs.push(format!("{at}: fewer than {m} items"));
            }
        }
        if let Some(m) = s.get("maxItems").and_then(|m| m.as_u64()) {
            if (arr.len() as u64) > m {
                errs.push(format!("{at}: more than {m} items"));
            }
        }
        if let Some(items) = s.get("items") {
            for (i, it) in arr.iter().enumerate() {
                check(root, items, it, &format!("{path}/{i}"), errs);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn theme_schema() -> Value {
        serde_json::from_str(include_str!("../../themes/theme.schema.json")).unwrap()
    }

    #[test]
    fn default_theme_is_valid() {
        let t: Value = serde_json::from_str(include_str!("../../themes/default/theme.json")).unwrap();
        validate(&theme_schema(), &t).unwrap();
        let l: Value = serde_json::from_str(include_str!("../../themes/default/layout.json")).unwrap();
        let ls: Value = serde_json::from_str(include_str!("../../themes/layout.schema.json")).unwrap();
        validate(&ls, &l).unwrap();
    }

    #[test]
    fn rejects_bad_themes() {
        let s = theme_schema();
        assert!(validate(&s, &json!({"name": "x", "version": "1"})).is_err()); // no apiVersion
        assert!(validate(&s, &json!({"name": "x", "version": "1", "apiVersion": 2})).is_err());
        let e = validate(&s, &json!({"name": "x", "version": "1", "apiVersion": 1, "colors": {"bg": "url(https://evil)"}})).unwrap_err();
        assert!(e[0].contains("/colors/bg"), "{e:?}");
        assert!(validate(&s, &json!({"name": "x", "version": "1", "apiVersion": 1, "evil": true})).is_err());
        assert!(validate(&s, &json!({"name": "x", "version": "1", "apiVersion": 1, "fontFaces": [{"family": "F", "src": "https://x/f.woff2"}]})).is_err());
    }
}
