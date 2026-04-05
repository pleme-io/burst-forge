-- DNS resolution latency during burst (requires Hubble flows).
-- Parameters: {experiment_id}
SELECT
  tumbling_window(timestamp_ms, 5000) as window_ms,
  COUNT(*) as dns_queries,
  AVG(CAST(http_code AS DOUBLE)) as avg_response_code
FROM events
WHERE signal_type = 'flow'
  AND l7_type = 'DNS'
  AND experiment_id = '{experiment_id}'
GROUP BY tumbling_window(timestamp_ms, 5000)
ORDER BY window_ms
