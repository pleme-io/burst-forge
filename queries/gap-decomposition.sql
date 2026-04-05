-- Gap decomposition: WHERE does time go between theoretical minimum and actual?
-- Parameters: {experiment_id}
SELECT
  event_type,
  MIN(timestamp_ms) as first_ms,
  MAX(timestamp_ms) as last_ms,
  MAX(timestamp_ms) - MIN(timestamp_ms) as duration_ms,
  COUNT(*) as event_count
FROM events
WHERE experiment_id = '{experiment_id}'
GROUP BY event_type
ORDER BY first_ms
