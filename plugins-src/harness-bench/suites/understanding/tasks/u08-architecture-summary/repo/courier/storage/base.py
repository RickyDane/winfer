class Store:
    def __init__(self, settings):
        self.settings = settings

    def put(self, key, value):
        raise NotImplementedError

    def get(self, key):
        raise NotImplementedError
