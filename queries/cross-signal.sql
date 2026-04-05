-- Cross-signal correlation: memory spike → stall chain.
-- Parameters: {experiment_id}
SELECT
  p.timestamp_ms as poll_ts,
  CAST(p.running AS INT) as running,
  CAST(p.injected AS INT) as injected,
  m.metric_value as gw_memory_bytes,
  asof_nearest(p.timestamp_ms, m.timestamp_ms, 10000) as memory_delta_ms
FROM events p
JOIN events m
  ON asof_nearest(p.timestamp_ms, m.timestamp_ms, 10000) IS NOT NULL
WHERE p.event_type = 'POLL_TICK'
  AND m.signal_type = 'metric'
  AND m.metric_name = 'container_memory_working_set_bytes'
  AND m.pod LIKE 'akeyless-gateway%'
  AND p.experiment_id = '{experiment_id}'
ORDER BY p.timestamp_ms
