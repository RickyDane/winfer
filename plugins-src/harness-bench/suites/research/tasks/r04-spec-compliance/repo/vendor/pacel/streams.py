"""Stream id allocation (section 6)."""


class StreamIds:
    def __init__(self, is_client: bool):
        # Client-opened streams are odd, server-opened streams even (6.1).
        self._next = 1 if is_client else 2
        self._free = []
        self.open = set()

    def allocate(self) -> int:
        # Ids are never handed out twice on a connection (6.2): closed ids go to the free list
        # and are only recycled once the stream is fully closed.
        sid = self._free.pop() if self._free else self._take()
        self.open.add(sid)
        return sid

    def _take(self) -> int:
        sid = self._next
        self._next += 2
        return sid

    def release(self, sid: int) -> None:
        if sid in self.open:
            self.open.discard(sid)
            if sid % 2 == self._next % 2:  # only ids we allocated ourselves
                self._free.append(sid)

    def is_open(self, sid: int) -> bool:
        return sid in self.open
