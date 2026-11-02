#!/usr/bin/env python3
"""Fail closed when the production environment lacks its initial review gate."""
import json
import os
import sys
import urllib.request


def validate(environment, policies):
    rules = environment.get("protection_rules", [])
    if not any(rule.get("type") == "required_reviewers" and rule.get("reviewers") for rule in rules):
        raise ValueError("Configure at least one required production reviewer")
    policy = environment.get("deployment_branch_policy") or {}
    if policy.get("protected_branches"):
        return
    names = {(item.get("name"), item.get("type", "branch")) for item in policies.get("branch_policies", [])}
    if not policy.get("custom_branch_policies") or names != {("main", "branch")}:
        raise ValueError("Restrict production deployment to protected branches or main only")


def main():
    try:
        def get(path):
            request = urllib.request.Request("https://api.github.com/repos/xcOnGithub/wealthfolio-private/environments/production" + path,
                                             headers={"Authorization": "Bearer " + os.environ["GH_TOKEN"], "Accept": "application/vnd.github+json"})
            with urllib.request.urlopen(request, timeout=20) as response:
                return json.load(response)
        environment = get("")
        policies = get("/deployment-branch-policies?per_page=100") if (environment.get("deployment_branch_policy") or {}).get("custom_branch_policies") else {}
        validate(environment, policies)
    except (ValueError, OSError, KeyError):
        print("Production approval/branch policy is missing or cannot be verified. Keep releases disabled until it is configured.", file=sys.stderr)
        return 1
    print("Production reviewer and branch restrictions verified")
    return 0


if __name__ == "__main__":
    sys.exit(main())
