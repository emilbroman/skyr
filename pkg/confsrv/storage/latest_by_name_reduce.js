function (_keys, candidates) {
  let latest;

  for (const candidate of candidates) {
    if (latest && latest.timestamp > candidate.timestamp) {
      continue;
    }
    latest = candidate;
  }

  return latest;
}
