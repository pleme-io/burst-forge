-- Connection reuse patterns: how many TCP connections per burst pod?
-- Parameters: {experiment_id}
SELECT
  src_pod,
  dst_pod,
  protocol,
  COUNT(*) as flow_count,
  verdict
FROM events
WHERE signal_type = 'flow'
  AND dst_pod LIKE 'akeyless-gateway%'
  AND experiment_id = '{experiment_id}'
GROUP BY src_pod, dst_pod, protocol, verdict
ORDER BY flow_count DESC
LIMIT 50
