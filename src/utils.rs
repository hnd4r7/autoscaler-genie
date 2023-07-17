use crate::controller::Error;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, LabelSelectorRequirement};
use std::collections::BTreeMap;
use tracing::error;

// Refer to: LabelSelectorAsSelector: https://github.com/kubernetes/kubernetes/blob/master/vendor/k8s.io/apimachinery/pkg/apis/meta/v1/helpers.go#L34
pub fn match_label(selector: &LabelSelector, labels: &BTreeMap<String, String>) -> bool {
    if let Some(match_labels) = &selector.match_labels {
        for (k, v) in match_labels {
            match labels.get(k) {
                None => return false,
                Some(x) => {
                    if x != v {
                        return false;
                    }
                }
            }
        }
    }
    for exp in selector.match_expressions.iter().flatten() {
        let matched = match exp.operator.as_str() {
            "IN" => labels
                .get(&exp.key)
                .map_or(false, |key| exp.values.as_ref().map_or(false, |v| v.contains(key))),
            "NotIn" => labels
                .get(&exp.key)
                .map_or(true, |key| exp.values.as_ref().map_or(true, |v| !v.contains(key))),
            "Exists" => labels.get(&exp.key).is_some(),
            "DoesNotExist" => labels.get(&exp.key).is_none(),
            op => {
                error!("LabelSelector has invalid/unknown operator [{op}]");
                false
            }
        };
        if !matched {
            return false;
        }
    }

    // for labels in selector.match_labels.map(|t|t.contains_key("")) {
    // }
    true
}

/// Takes a [`LabelSelector`] and converts it to a String that can be used in Kubernetes API calls.
/// It will return an error if the LabelSelector contains illegal things (e.g. an `Exists` operator
/// with a value).
pub fn convert_label_selector_to_query_string(
    label_selector: &LabelSelector,
) -> Result<String, Error> {
    let mut query_string = String::new();

    // match_labels are the "old" part of LabelSelectors.
    // They are the equivalent for the "In" operator in match_expressions
    // In a query string each key-value pair will be separated by an "=" and the pairs
    // are then joined on commas.
    // The whole match_labels part is optional so we only do this if there are match labels.
    if let Some(label_map) = &label_selector.match_labels {
        query_string.push_str(
            &label_map
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(","),
        );
    }

    // Match expressions are more complex than match labels, both can appear in the same API call
    // They support these operators: "In", "NotIn", "Exists" and "DoesNotExist"
    let expressions = label_selector.match_expressions.as_ref().map(|requirements| {
        // If we had match_labels AND we have match_expressions we need to separate those two
        // with a comma.
        if !requirements.is_empty() && !query_string.is_empty() {
            query_string.push(',');
        }

        // Here we map over all requirements (which might be empty) and for each of the requirements
        // we create a Result<String, Error> with the Ok variant being the converted match expression
        // We then collect those Results into a single Result with the Error being the _first_ error.
        // This, unfortunately means, that we'll throw away all but one error.
        // TODO: Return all errors in one go: https://github.com/stackabletech/operator-rs/issues/127
        let expression_string: Result<Vec<String>, Error> = requirements
            .iter()
            .map(|requirement| match requirement.operator.as_str() {
                // In and NotIn can be handled the same, they both map to a simple "key OPERATOR (values)" string
                operator @ "In" | operator @ "NotIn" => match &requirement.values {
                    Some(values) if !values.is_empty() => Ok(format!(
                        "{} {} ({})",
                        requirement.key,
                        operator.to_ascii_lowercase(),
                        values.join(", ")
                    )),
                    _ => Err(Error::InvalidLabelSelector(
                        "LabelSelector has no or empty values for [{operator}] operator".into(),
                    )),
                },
                // "Exists" is just the key and nothing else, if values have been specified it's an error
                "Exists" => match &requirement.values {
                    Some(values) if !values.is_empty() => Err(Error::InvalidLabelSelector(
                        "LabelSelector has [Exists] operator with values, this is not legal"
                            .to_string(),
                    )),
                    _ => Ok(requirement.key.to_string()),
                },
                // "DoesNotExist" is similar to "Exists" but it is preceded by an exclamation mark
                "DoesNotExist" => match &requirement.values {
                    Some(values) if !values.is_empty() => Err(Error::InvalidLabelSelector(
                        "LabelSelector has [DoesNotExist] operator with values, this is not legal"
                            .to_string(),
                    )),
                    _ => Ok(format!("!{}", requirement.key)),
                },
                op => Err(Error::InvalidLabelSelector(format!(
                    "LabelSelector has illegal/unknown operator [{op}]"
                ))),
            })
            .collect();

        expression_string
    });

    if let Some(expressions) = expressions.transpose()? {
        query_string.push_str(&expressions.join(","));
    };

    Ok(query_string)
}

mod test {
    use std::collections::BTreeMap;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
    use crate::utils::match_label;

    #[test]
    fn test_match_labels() {
        let selector = LabelSelector {
            match_labels: Some(std::collections::BTreeMap::from([("app".into(), "lll".into())])),
            ..Default::default()
        };
        let labels = BTreeMap::from([("app".to_string(), "lll".to_string())]);
        assert!(match_label(&selector, &labels));

        let labels = BTreeMap::from([("app".to_string(), "ll".to_string())]);
        assert!(!match_label(&selector, &labels));
    }
}
