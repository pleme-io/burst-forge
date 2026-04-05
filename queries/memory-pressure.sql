-- GW memory pressure: is the gateway hitting its memory limit?
-- Parameters: {experiment_id}
SELECT
  tumbling_window(timestamp_ms, 5000) as window_ms,
  AVG(metric_value) as avg_memory_bytes,
  MAX(metric_value) as peak_memory_bytes,
  COUNT(*) as sample_count
FROM events
WHERE signal_type = 'metric'
  AND metric_name = 'container_memory_working_set_bytes'
  AND pod LIKE 'akeyless-gateway%'
  AND experiment_id = '{experiment_id}'
GROUP BY tumbling_window(timestamp_ms, 5000)
ORDER BY window_ms
