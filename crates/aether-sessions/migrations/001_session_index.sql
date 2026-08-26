create table session_files (
  source_path text primary key,
  session_id text unique,
  status text not null,
  error text,
  file_size integer not null,
  file_mtime_ns integer not null,
  indexed_at text not null,
  event_count integer not null default 0,
  parse_error_count integer not null default 0,
  cwd text,
  model text,
  selected_mode text,
  created_at text
);

create table events (
  session_id text not null,
  event_index integer not null,
  line_number integer not null,
  turn_index integer,
  content text,
  content_len integer not null default 0,
  raw_json text not null,
  kind text not null,
  event_type text not null,
  outcome text,
  tool_call_id text,
  tool_name text,
  tool_arguments text,
  model_name text,
  message_id text,
  usage_ratio real,
  context_limit integer,
  input_tokens integer,
  output_tokens integer,
  cache_read_tokens integer,
  cache_creation_tokens integer,
  reasoning_tokens integer,
  total_input_tokens integer,
  total_output_tokens integer,
  total_cache_read_tokens integer,
  total_cache_creation_tokens integer,
  total_reasoning_tokens integer,
  primary key (session_id, event_index),
  foreign key (session_id) references session_files(session_id) on delete cascade
);

create table parse_errors (
  id integer primary key autoincrement,
  source_path text not null,
  session_id text,
  line_number integer,
  error text not null,
  line_excerpt text
);

create index events_kind_type_idx on events(kind, event_type);
create index events_tool_name_idx on events(tool_name);
create index events_session_turn_idx on events(session_id, turn_index);
create index events_usage_ratio_idx on events(usage_ratio);
create index events_outcome_idx on events(outcome);
create index session_files_cwd_idx on session_files(cwd);
create index session_files_created_at_idx on session_files(created_at);

create view sessions as select * from session_files where status = 'indexed';
create view user_messages as select * from events where kind = 'user' and event_type = 'user_message';
create view agent_messages as select * from events where kind = 'agent' and event_type in ('message_text', 'message_thought');
create view tool_calls as select * from events where event_type = 'tool_call';
create view tool_results as select * from events where event_type = 'tool_result';
create view tool_errors as select * from events where event_type = 'tool_error';
create view context_usage as select * from events where event_type = 'context_usage';
create view retries as select * from events where event_type = 'retry_scheduled';
create view cancellations as select * from events where outcome = 'cancelled';
