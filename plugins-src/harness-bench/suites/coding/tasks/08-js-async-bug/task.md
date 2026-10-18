`node --test` fails: the async helpers in src/async.js (`mapLimit` and `retry`) have bugs. Fix them so they behave as documented in README.md and the tests pass. Don't change the tests.
