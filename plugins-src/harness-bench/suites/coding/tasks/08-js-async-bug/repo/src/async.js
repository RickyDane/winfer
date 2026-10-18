// Small promise helpers. See README.md.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function mapLimit(items, limit, fn) {
  const results = [];
  let next = 0;
  let active = 0;
  return new Promise((resolve, reject) => {
    const launch = () => {
      while (active < limit && next < items.length) {
        const i = next++;
        active++;
        fn(items[i], i).then((value) => {
          results.push(value);
          active--;
          if (results.length === items.length) resolve(results);
          else launch();
        });
      }
    };
    launch();
  });
}

export async function retry(fn, { retries = 3, delayMs = 0 } = {}) {
  let attempt = 1;
  let lastError;
  while (attempt < retries) {
    try {
      return fn(attempt);
    } catch (err) {
      lastError = err;
      attempt++;
      await sleep(delayMs);
    }
  }
  throw lastError;
}
