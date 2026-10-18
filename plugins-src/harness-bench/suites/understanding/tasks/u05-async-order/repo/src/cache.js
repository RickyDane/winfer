'use strict';
const { log } = require('./log');
const { schedule } = require('./scheduler');

let readyResolve;
const readyPromise = new Promise((r) => { readyResolve = r; });

async function load() {
  log('cache:load');
  return 42;
}

async function warm() {
  log('cache:begin');
  const value = await load();
  schedule(() => log('cache:tick'));
  queueMicrotask(() => log('cache:micro'));
  readyResolve('ready');
  log('cache:end');
  return Promise.resolve(value);
}

function ready() {
  return readyPromise.then((v) => { log('cache:ready'); return v; });
}

module.exports = { warm, ready };
