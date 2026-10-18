'use strict';

const seen = [];

function log(label) {
  seen.push(label);
  console.log(label);
}

module.exports = { log, seen };
