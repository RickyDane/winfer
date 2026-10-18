// Small promise helpers. See README.md.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function mapLimit(items, limit, fn) {
  const results = new Array(items.length);
  let next = 0;
  let active = 0;
  let done = 0;
  let failed = false;
  return new Promise((resolve, reject) => {
    if (items.length === 0) return resolve(results);
    const launch = () => {
      while (!failed && active < limit && next < items.length) {
        const i = next++;
        active++;
        Promise.resolve()
          .then(() => fn(items[i], i))
          .then(
            (value) => {
              active--;
              if (failed) return;
              results[i] = value;
              done++;
              if (done === items.length) resolve(results);
              else launch();
            },
            (err) => {
              active--;
              if (failed) return;
              failed = true;
              reject(err);
            },
          );
      }
    };
    launch();
  });
}

export async function retry(fn, { retries = 3, delayMs = 0 } = {}) {
  let lastError;
  for (let attempt = 1; attempt <= retries + 1; attempt++) {
    try {
      return await fn(attempt);
    } catch (err) {
      lastError = err;
      if (attempt <= retries) await sleep(delayMs);
    }
  }
  throw lastError;
}
