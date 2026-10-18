from vendor.pacel.frame import Frame, decode, encode


def test_roundtrip():
    f = Frame(3, 1, b"hello")
    out, used = decode(encode(f))
    assert out == f and used == len(encode(f))


def test_reserved_flags_cleared():
    raw = encode(Frame(3, 1, b"x", flags=0xFF))
    assert raw[4] == 0x0F
