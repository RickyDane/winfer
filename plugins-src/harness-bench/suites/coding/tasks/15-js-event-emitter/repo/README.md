# Emitter

A small synchronous event emitter. `src/emitter.js` exports `class Emitter`.

```js
import { Emitter } from "./src/emitter.js";
const e = new Emitter({ maxListeners: 10, warn: console.warn });
```

## Constructor

`new Emitter(options?)`

- `maxListeners` (default `10`): when adding a listener makes the number of listeners for one
  event name exceed this, `warn(message)` is called **once per event name** (not again for
  later additions to the same name, even after removals). `0` means unlimited. The message
  must contain the event name.
- `warn` (default `console.warn`): function used for that warning.

## Methods

- `on(name, fn)` — adds `fn` to the end of the listener list for `name`. Adding the same
  function twice registers it twice. Returns `this`. Throws `TypeError` if `fn` is not a function.
- `once(name, fn)` — like `on`, but the listener is removed before it is first called, so it
  runs at most once even if it emits `name` again from inside itself. Returns `this`.
- `off(name, fn)` — removes the **most recently added** registration of `fn` for `name`
  (works for listeners added with `once` too, passing the original function). Removing a
  function that isn't registered does nothing. `off(name)` with no function removes all
  listeners for `name`. `off()` with no arguments removes every listener. Returns `this`.
- `emit(name, ...args)` — calls the listeners for `name` in the order they were added, with
  `args`, and `this` set to the emitter. Then calls the wildcard listeners (see below).
  Returns `true` if at least one listener (named or wildcard) was called, else `false`.
- `listenerCount(name)` — number of listeners registered for `name` (wildcard listeners are
  not counted, except for `listenerCount("*")`).
- `listeners(name)` — a new array of the original functions (for `once`, the function that was
  passed in, not a wrapper), in call order.
- `eventNames()` — array of names that currently have at least one listener, in the order each
  name first got a listener (a name that lost all listeners and got a new one goes to the end).

## Wildcard

Listeners registered on `"*"` are called after the named listeners for **every** emitted
event, with `(name, ...args)`. Emitting `"*"` directly calls the wildcard listeners only once,
with `("*", ...args)`.

## Changes during emit

`emit` works on a snapshot of the listener list taken when it starts: a listener added during
an emit is not called by that emit; a listener removed during an emit (that hasn't run yet)
**is still called** by that emit. (The wildcard snapshot is taken at the same time.)

## The "error" event

If `"error"` is emitted and there are no `"error"` listeners (wildcard listeners don't count),
`emit` throws: the first argument if it is an `Error`, otherwise a new `Error` whose message
contains `String(firstArgument)`. Wildcard listeners are not called in that case.

If a listener throws, the exception propagates out of `emit` and the remaining listeners for
that emit are not called.
