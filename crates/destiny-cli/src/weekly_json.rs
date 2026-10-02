//! Weekly JSON renderer, also exercised by the MCP parity fixture.
use serde_json::{Value, json};

struct WeeklyJsonAdapter;
impl destiny_app::weekly_projection::WeeklyJson for WeeklyJsonAdapter {
    type Value = Value;
    fn object(fields: Vec<(&'static str, Value)>) -> Value {
        Value::Object(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }
    fn array(values: Vec<Value>) -> Value {
        Value::Array(values)
    }
    fn string(value: &str) -> Value {
        json!(value)
    }
    fn integer(value: i64) -> Value {
        json!(value)
    }
    fn boolean(value: bool) -> Value {
        json!(value)
    }
    fn null() -> Value {
        Value::Null
    }
}

pub fn render_weekly_json(
    character: &destiny_app::Character,
    weekly: &destiny_app::WeeklyPersonal,
) -> String {
    destiny_app::weekly_projection::weekly_projection::<WeeklyJsonAdapter>(character, weekly)
        .to_string()
}
