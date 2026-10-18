from vendor.pacel import Connection


class T:
    def __init__(self):
        self.out = []

    def write(self, b):
        self.out.append(b)

    def close(self):
        pass


def test_client_sends_hello_first():
    t = T()
    Connection(t, is_client=True)
    assert t.out[0][3] == 1
