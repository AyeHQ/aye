import unittest

from tests.shared import BaseCase, SharedTests


class Mixin:
    def test_from_mixin(self):
        self.check_positive(1)


class Base(BaseCase):
    def test_base(self):
        self.check_positive(2)

    def test_overridden(self):
        self.fail("replaced in Child")


class Child(Mixin, Base):
    def test_overridden(self):
        self.check_positive(3)

    def test_child(self):
        self.check_positive(-1)


class AsyncCase(unittest.IsolatedAsyncioTestCase):
    async def test_async(self):
        self.assertEqual(await self.value(), 1)

    async def value(self):
        return 1
