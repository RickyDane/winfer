'use strict';
const fs = require('fs');

// Client certificate for mutual TLS. Read once at startup.
const CERT = fs.readFileSync('/etc/inventory/client.pem');
const KEY = fs.readFileSync('/etc/inventory/client.key');

module.exports = { CERT, KEY };
