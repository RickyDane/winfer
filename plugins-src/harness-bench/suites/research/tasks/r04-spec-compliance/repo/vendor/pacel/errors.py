class ProtocolError(Exception):
    """The peer sent something the spec does not allow."""


class FrameTooLarge(ProtocolError):
    pass


class ChecksumMismatch(ProtocolError):
    pass


class ConnectionClosed(Exception):
    pass
