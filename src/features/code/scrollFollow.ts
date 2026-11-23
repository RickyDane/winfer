/** Pausing follow alone does not mean there is anything below the viewport. */
export function isAwayFromBottom(following: boolean, distance: number): boolean {
  return !following && distance > 2;
}

/** A scroll event is not necessarily user input: anchoring, history trimming,
 * and smooth scrolling also emit it. Input handlers pause follow before scrolling;
 * layout-generated upward movement must not pause it again. Content can shrink
 * and grow before the event arrives, leaving a nonzero distance from the bottom.
 * Only a downward return to the bottom may resume follow after it was paused. */
export function followAfterScroll(
  following: boolean,
  previousTop: number,
  top: number,
  distance: number,
  intent: number,
): boolean {
  if (!following)
    return intent >= 0 && top > previousTop && distance <= 2;
  return true;
}

/** Whether a nested scroll box (tool output, diff) can still move in the wheel's direction.
 * WebKit latches a trackpad gesture to the box under the pointer, so once it reaches its end
 * the transcript would not move until the pointer leaves it; the caller then scrolls it instead. */
export function canScrollFurther(top: number, height: number, client: number, delta: number): boolean {
  if (height - client <= 1) return false;
  return delta < 0 ? top > 0.5 : top + client < height - 0.5;
}
