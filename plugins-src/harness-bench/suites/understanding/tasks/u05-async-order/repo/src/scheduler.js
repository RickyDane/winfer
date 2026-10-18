'use strict';
const { log } = require('./log');

// Runs `fn` "as soon as possible" after the current operation.
function schedule(fn) {
  process.nextTick(fn);
}

// Runs `fn` after everything currently queued, including timers.
function later(label) {
  return new Promise((resolve) => setTimeout(() => { log(label); resolve(label); }, 0));
}

module.exports = { schedule, later };
