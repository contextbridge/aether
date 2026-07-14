use serde::Serialize;

#[derive(Serialize)]
pub struct SchemaDoc {
    pub schema_sql: &'static str,
    pub examples: Vec<Example>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::query::{QueryOptions, run_query};
    use tempfile::TempDir;

    #[tokio::test]
    async fn documented_queries_execute_against_current_schema() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("index.sqlite");
        drop(Db::open_writable(&db_path).await.unwrap());

        for example in examples() {
            run_query(&QueryOptions {
                db_path: db_path.clone(),
                sql: example.sql.to_string(),
                max_rows: 100,
                max_cell_chars: 1000,
                timeout_ms: 2000,
            })
            .await
            .unwrap_or_else(|error| panic!("example '{}' failed: {error}", example.name));
        }
    }
}

#[derive(Serialize)]
pub struct Example {
    pub name: &'static str,
    pub sql: &'static str,
}

pub fn schema_doc() -> SchemaDoc {
    SchemaDoc { schema_sql: include_str!("../migrations/001_session_index.sql"), examples: examples() }
}

pub fn render_schema_text(schema: &SchemaDoc) -> String {
    let mut text = String::from(schema.schema_sql);
    text.push_str("\nExamples:\n");
    for example in &schema.examples {
        text.push_str("-- ");
        text.push_str(example.name);
        text.push('\n');
        text.push_str(example.sql);
        text.push_str(";\n\n");
    }
    text
}

fn examples() -> Vec<Example> {
    vec![
        Example {
            name: "Most failing tools",
            sql: "select tool_name, count(*) as failures from tool_errors group by tool_name order by failures desc limit 20",
        },
        Example {
            name: "Most used tools",
            sql: "select tool_name, count(*) as calls from tool_calls group by tool_name order by calls desc limit 20",
        },
        Example {
            name: "Tool error rate by tool",
            sql: "with calls as (select tool_name, count(*) as calls from tool_calls group by tool_name), errors as (select tool_name, count(*) as errors from tool_errors group by tool_name) select calls.tool_name, calls.calls, coalesce(errors.errors, 0) as errors, round(100.0 * coalesce(errors.errors, 0) / calls.calls, 2) as error_rate_pct from calls left join errors using (tool_name) order by error_rate_pct desc, calls.calls desc",
        },
        Example {
            name: "Sessions with high context usage",
            sql: "select s.session_id, s.cwd, max(e.usage_ratio) as max_usage_ratio from context_usage e join sessions s using (session_id) group by s.session_id, s.cwd having max_usage_ratio > 0.8 order by max_usage_ratio desc",
        },
        Example {
            name: "Malformed event lines",
            sql: "select source_path, line_number, error, line_excerpt from parse_errors order by id desc limit 50",
        },
    ]
}
