'use strict';
const tls = require('tls');
const { CERT, KEY } = require('./tls');
const { log } = require('../log');

function createClient(host = process.env.INVENTORY_HOST) {
  return {
    publish(snapshot) {
      log('publish:sent');
      return new Promise((resolve, reject) => {
        const socket = tls.connect({ host, port: 9443, cert: CERT, key: KEY }, () => {
          socket.end(JSON.stringify(snapshot));
        });
        socket.on('data', (d) => resolve(JSON.parse(d)));
        socket.on('error', reject);
      });
    },
  };
}

module.exports = { createClient };
