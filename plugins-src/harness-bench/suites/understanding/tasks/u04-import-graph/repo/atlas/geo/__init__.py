from .shapes import Shape

try:
    from .projection import mercator
except ImportError:  # projection support is optional
    mercator = None
