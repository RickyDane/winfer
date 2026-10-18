from ..sanitize import quote
from ..services import users as service


def update_profile(request):
    body = request.json
    return service.rename(request.user_id, display_name=body["display_name"], email=body.get("email"))


def list_users(request):
    team = request.args.get("team", "")
    return service.list_users(team)
