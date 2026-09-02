alter table events add column usage_sequence integer;
alter table events add column agent_id text;
alter table events add column parent_agent_id text;
alter table events add column task_id text;
alter table events add column agent_name text;
alter table events add column call_purpose text;
alter table events add column provider text;
alter table events add column estimated_cost_usd real;
alter table events add column total_estimated_cost_usd real;
alter table events add column unpriced_calls integer;

create index events_session_usage_sequence_idx on events(session_id, usage_sequence);
create index events_agent_id_idx on events(agent_id);
create view session_usage as select * from events where event_type = 'session_usage';
