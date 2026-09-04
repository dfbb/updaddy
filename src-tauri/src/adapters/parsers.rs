use serde_json::Value;

pub fn json_object_versions(value: &str) -> Result<Vec<(String, String, Option<String>)>, String> {
    let root: Value = serde_json::from_str(value).map_err(|error| error.to_string())?;
    let object = root
        .as_object()
        .ok_or_else(|| "expected JSON object".to_owned())?;
    let mut rows = Vec::new();
    for (name, entry) in object {
        let current = entry
            .get("version")
            .and_then(Value::as_str)
            .or_else(|| entry.get("current").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        let target = entry
            .get("latest")
            .and_then(Value::as_str)
            .or_else(|| entry.get("wanted").and_then(Value::as_str))
            .map(str::to_owned);
        rows.push((name.clone(), current, target));
    }
    Ok(rows)
}

pub fn json_array_versions(value: &str) -> Result<Vec<(String, String, Option<String>)>, String> {
    let root: Value = serde_json::from_str(value).map_err(|error| error.to_string())?;
    let array = root
        .as_array()
        .ok_or_else(|| "expected JSON array".to_owned())?;
    Ok(array
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("name")?.as_str()?.to_owned(),
                entry
                    .get("version")
                    .or_else(|| entry.get("current_version"))?
                    .as_str()?
                    .to_owned(),
                entry
                    .get("latest_version")
                    .or_else(|| entry.get("latest"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ))
        })
        .collect())
}

pub fn lines(value: &str) -> impl Iterator<Item = &str> {
    value.lines().map(str::trim).filter(|line| !line.is_empty())
}
