from . import events
from .registry import on

FEATURE_GDPR = False


@on(events.USER_SIGNUP)
def user_signup(msg):
    return {"notify": "crm", "user": msg["id"]}


if FEATURE_GDPR:

    @on("user.deleted")
    def user_deleted(msg):
        return {"notify": "crm", "forget": msg["id"]}
