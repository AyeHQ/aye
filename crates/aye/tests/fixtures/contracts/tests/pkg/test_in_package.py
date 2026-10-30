"""A module in a package, so that the conftest sees a package's nodes."""


class TestInClass:
    def test_method(self, hold, request):
        hold("a method's request.node", request.node)
        hold("a method's request.node.parent", request.node.parent)
