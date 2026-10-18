'use strict';
const { log } = require('./log');
const cache = require('./cache');
const { createClient } = require('./net/client');
const { schedule, later } = require('./scheduler');

async function run() {
  log('run:start');
  const client = createClient();
  setTimeout(() => log('timer:a'), 1);
  schedule(() => log('scheduled'));
  const warming = cache.warm();
  Promise.resolve().then(() => log('micro:1')).then(() => log('micro:2'));
  setTimeout(() => log('timer:b'), 0);
  await null;
  log('run:after-await');
  const value = await warming;
  log('run:warm');
  const pub = client.publish({ value });
  pub.then(() => log('publish:ok'), () => log('publish:failed'));
  const winner = await Promise.race([pub, cache.ready(), later('race:timer')]);
  log('run:raced:' + winner);
  await later('later:2');
  await pub;
  log('run:done');
}

run().finally(() => log('finally'));
log('main:end');
