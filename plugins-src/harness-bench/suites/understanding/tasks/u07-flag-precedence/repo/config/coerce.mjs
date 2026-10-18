const TRUE = new Set(['1', 'true', 'yes', 'on']);

export function parseBool(value) {
  return TRUE.has(String(value).trim().toLowerCase());
}

export function coerce(value, fallback) {
  switch (typeof fallback) {
    case 'boolean':
      return parseBool(value);
    case 'number': {
      const n = Number(value);
      return Number.isFinite(n) ? n : fallback;
    }
    default:
      return String(value);
  }
}

/** newCheckout -> NEW_CHECKOUT */
export function envName(key) {
  return 'FLAG_' + key.replace(/([A-Z])/g, '_$1').toUpperCase();
}
