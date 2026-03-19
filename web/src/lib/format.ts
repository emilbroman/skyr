/**
 * Reformats the serde-serialized sclc::Value JSON into a compact, human-readable form.
 *
 * The Rust backend serializes sclc::Value using serde's default externally-tagged enum format:
 *   {"Str": "hello"}, {"Int": 42}, {"Bool": true}, "Nil", {"List": [...]},
 *   {"Record": {"fields": {...}}}, {"Dict": {"entries": [...]}},
 *   {"Float": 3.14}, {"Pending": ...}, {"Fn": ...}, {"ExternFn": ...}, {"Exception": ...}
 *
 * A top-level sclc::Record serializes as {"fields": {"key": <Value>, ...}}.
 *
 * This module converts those into plain JSON:
 *   "hello", 42, true, null, [...], {...}, 3.14, "<pending>", "<function>", "<exception>"
 */

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;

/**
 * Format a top-level sclc::Record (the shape of resource inputs/outputs).
 * Input: {"fields": {"key": {"Str": "val"}, ...}} or already-plain JSON.
 * Output: {"key": "val", ...}
 */
export function formatRecord(raw: Json): Json {
	if (raw == null) return null;
	if (typeof raw === 'object' && !Array.isArray(raw) && 'fields' in raw && typeof raw.fields === 'object') {
		return formatRecordFields(raw.fields);
	}
	// Already plain or unrecognized — try formatting as a value
	return formatValue(raw);
}

function formatRecordFields(fields: Record<string, Json>): Record<string, Json> {
	const out: Record<string, Json> = {};
	for (const [key, val] of Object.entries(fields)) {
		out[key] = formatValue(val);
	}
	return out;
}

/**
 * Format a single sclc::Value from its serde JSON representation.
 */
export function formatValue(raw: Json): Json {
	if (raw == null) return null;

	// Primitives that are already plain JSON (number, boolean, string that isn't a tag)
	if (typeof raw === 'number' || typeof raw === 'boolean') return raw;

	// "Nil" serializes as the string "Nil"
	if (raw === 'Nil') return null;

	// Other string tags for non-serializable variants
	if (typeof raw === 'string') return raw;

	// Arrays — shouldn't appear at this level in tagged-enum form, but handle gracefully
	if (Array.isArray(raw)) return raw.map(formatValue);

	// Tagged enum object: single key determines the variant
	const keys = Object.keys(raw);
	if (keys.length === 1) {
		const tag = keys[0];
		const inner = raw[tag];

		switch (tag) {
			case 'Str':
				return inner;
			case 'Int':
				return inner;
			case 'Float':
				return inner;
			case 'Bool':
				return inner;
			case 'List':
				return Array.isArray(inner) ? inner.map(formatValue) : inner;
			case 'Record':
				if (inner && typeof inner === 'object' && 'fields' in inner) {
					return formatRecordFields(inner.fields);
				}
				return inner;
			case 'Dict':
				if (inner && typeof inner === 'object' && 'entries' in inner && Array.isArray(inner.entries)) {
					const obj: Record<string, Json> = {};
					for (const entry of inner.entries) {
						if (Array.isArray(entry) && entry.length === 2) {
							const k = formatValue(entry[0]);
							const v = formatValue(entry[1]);
							obj[String(k)] = v;
						}
					}
					return obj;
				}
				return inner;
			case 'Nil':
				return null;
			case 'Pending':
				return '<pending>';
			case 'Fn':
				return '<function>';
			case 'ExternFn':
				return '<function>';
			case 'Exception':
				return '<exception>';
		}
	}

	// Unrecognized object — recurse on values
	const out: Record<string, Json> = {};
	for (const [key, val] of Object.entries(raw)) {
		out[key] = formatValue(val);
	}
	return out;
}
