#!/usr/bin/env node
import { parseArgs, ArgsError } from "../src/args.js";

try {
  const opts = parseArgs(process.argv.slice(2), {
    name: { type: "string", alias: "n", default: "world" },
    times: { type: "number", alias: "t", default: 1 },
    shout: { type: "boolean", alias: "s" },
  });
  for (let i = 0; i < opts.times; i++) {
    const msg = `hello, ${opts.name}`;
    console.log(opts.shout ? msg.toUpperCase() : msg);
  }
} catch (e) {
  if (e instanceof ArgsError) {
    console.error(e.message);
    process.exit(2);
  }
  throw e;
}
