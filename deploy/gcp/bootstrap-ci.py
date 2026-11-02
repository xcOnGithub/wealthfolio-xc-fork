#!/usr/bin/env python3
"""One-time, operator-run WIF/IAM setup. Defaults to printing a nonsecret plan."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import sys

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("manage", HERE / "manage.py")
manage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(manage)
REPOSITORY = "xcOnGithub/wealthfolio-private"
POOL = "wealthfolio-github"


def trust_condition(repository_id, owner_id, environment):
    if not str(repository_id).isdigit() or not str(owner_id).isdigit() or environment not in {"build", "production"}:
        raise ValueError("Numeric repository/owner IDs and a known environment are required")
    return (
        f"assertion.repository_id == '{repository_id}' && assertion.repository_owner_id == '{owner_id}' && "
        f"assertion.ref == 'refs/heads/main' && assertion.environment == '{environment}' && "
        f"assertion.workflow_ref == '{REPOSITORY}/.github/workflows/gcp-release.yml@refs/heads/main' && "
        "(assertion.event_name == 'push' || assertion.event_name == 'workflow_dispatch')"
    )


def plan(args):
    if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", args.project) or not str(args.project_number).isdigit():
        raise ValueError("Invalid project identity")
    if not re.fullmatch(r"[a-z][a-z0-9-]{0,62}", args.instance) or not (re.fullmatch(r"[a-z0-9-]+@" + re.escape(args.project) + r"\.iam\.gserviceaccount\.com", args.runtime_account) or args.runtime_account == str(args.project_number) + "-compute@developer.gserviceaccount.com"):
        raise ValueError("Verify the VM and its existing service account")
    if not re.fullmatch(r"[a-z]+-[a-z]+[0-9]+", args.region) or not re.fullmatch(re.escape(args.region) + r"-[a-z]", args.zone):
        raise ValueError("Zone must belong to the selected region")
    commands = []
    builder = f"wealthfolio-gh-build@{args.project}.iam.gserviceaccount.com"
    deployer = f"wealthfolio-gh-deploy@{args.project}.iam.gserviceaccount.com"
    commands.append(["services", "enable", "iam.googleapis.com", "iamcredentials.googleapis.com", "sts.googleapis.com", "artifactregistry.googleapis.com", "iap.googleapis.com", "oslogin.googleapis.com", "compute.googleapis.com"])
    for identity in ("wealthfolio-gh-build", "wealthfolio-gh-deploy"):
        commands.append(["ensure-account", identity])
    commands.append(["ensure-pool", POOL])
    providers = {}
    for provider, role, environment, account in [("github-build", "build", "build", builder), ("github-production", "deploy", "production", deployer)]:
        condition = trust_condition(args.repository_id, args.owner_id, environment)
        commands.append(["ensure-provider", provider, role, condition])
        principal = f"principalSet://iam.googleapis.com/projects/{args.project_number}/locations/global/workloadIdentityPools/{POOL}/attribute.pipeline_role/{role}"
        commands.append(["iam", "service-accounts", "add-iam-policy-binding", account, "--member=" + principal, "--role=roles/iam.workloadIdentityUser"])
        providers[role] = f"projects/{args.project_number}/locations/global/workloadIdentityPools/{POOL}/providers/{provider}"
    for account, role in [(builder, "roles/artifactregistry.writer"), (deployer, "roles/artifactregistry.reader")]:
        commands.append(["artifacts", "repositories", "add-iam-policy-binding", "wealthfolio", "--location", args.region, "--member=serviceAccount:" + account, "--role=" + role])
    commands.append(["compute", "instances", "add-iam-policy-binding", args.instance, "--zone", args.zone,
                     "--member=serviceAccount:" + deployer, "--role=roles/compute.osAdminLogin"])
    commands.append(["iap", "tcp", "add-iam-policy-binding", "--instance", args.instance, "--zone", args.zone,
                     "--member=serviceAccount:" + deployer, "--role=roles/iap.tunnelResourceAccessor",
                     "--condition=expression=destination.port == 22,title=WealthfolioSSH"])
    commands.append(["iam", "service-accounts", "add-iam-policy-binding", args.runtime_account,
                     "--member=serviceAccount:" + deployer, "--role=roles/iam.serviceAccountUser"])
    commands.append(["ensure-lookup-role"])
    commands.append(["ensure-iap-firewall"])
    commands.append(["projects", "add-iam-policy-binding", args.project, "--member=serviceAccount:" + deployer,
                     f"--role=projects/{args.project}/roles/wealthfolioProjectLookup", "--condition=None"])
    variables = {"WF_GCP_PROJECT": args.project, "WF_GCP_INSTANCE": args.instance, "WF_GCP_ZONE": args.zone,
                 "WF_GCP_IMAGE": f"{args.region}-docker.pkg.dev/{args.project}/wealthfolio/server",
                 "WF_GCP_BUILD_PROVIDER": providers["build"], "WF_GCP_DEPLOY_PROVIDER": providers["deploy"],
                 "WF_GCP_BUILD_ACCOUNT": builder, "WF_GCP_DEPLOY_ACCOUNT": deployer, "WF_GCP_RELEASE_ENABLED": "false"}
    return commands, variables


def apply(args, commands):
    # Deliberately refuse to change SSH authentication or the VM's identity.
    vm = manage.resource(["compute", "instances", "describe", args.instance, "--zone", args.zone], args.project)
    project = manage.resource(["projects", "describe", args.project], args.project)
    if str(project["projectNumber"]) != str(args.project_number):
        raise ValueError("Project number does not match the selected project")
    network = vm["networkInterfaces"][0]["network"]
    if "/projects/" + args.project + "/" not in network:
        raise ValueError("Shared-VPC firewall changes require the network owner's setup")
    metadata = {item["key"]: item["value"] for item in vm.get("metadata", {}).get("items", [])}
    if metadata.get("enable-oslogin", "").upper() != "TRUE" or [account["email"] for account in vm.get("serviceAccounts", [])] != [args.runtime_account]:
        raise ValueError("Verify operator recovery access, enable OS Login, and confirm the VM identity before bootstrap")
    repository = manage.resource(["artifacts", "repositories", "describe", "wealthfolio", "--location", args.region], args.project)
    if not repository.get("dockerConfig", {}).get("immutableTags"):
        raise ValueError("Prepare an immutable-tag Docker repository before bootstrap")
    for command in commands:
        if command[0] == "ensure-account":
            account = command[1] + "@" + args.project + ".iam.gserviceaccount.com"
            if manage.resource(["iam", "service-accounts", "describe", account], args.project, optional=True) is None:
                manage.gcloud(["iam", "service-accounts", "create", command[1]], args.project)
        elif command[0] == "ensure-pool":
            if manage.resource(["iam", "workload-identity-pools", "describe", POOL, "--location=global"], args.project, optional=True) is None:
                manage.gcloud(["iam", "workload-identity-pools", "create", POOL, "--location=global"], args.project)
        elif command[0] == "ensure-provider":
            _, provider, role, condition = command
            prefix = ["iam", "workload-identity-pools", "providers"]
            options = ["--location=global", "--workload-identity-pool=" + POOL]
            exists = manage.resource([*prefix, "describe", provider, *options], args.project, optional=True)
            manage.gcloud([*prefix, "update-oidc" if exists else "create-oidc", provider, *options,
                           "--issuer-uri=https://token.actions.githubusercontent.com",
                           "--attribute-mapping=google.subject=assertion.sub,attribute.pipeline_role='" + role + "'",
                           "--attribute-condition=" + condition], args.project)
        elif command[0] == "ensure-lookup-role":
            exists = manage.resource(["iam", "roles", "describe", "wealthfolioProjectLookup"], args.project, optional=True)
            manage.gcloud(["iam", "roles", "update" if exists else "create", "wealthfolioProjectLookup",
                           "--title=Wealthfolio project lookup", "--permissions=compute.projects.get,serviceusage.services.use", "--stage=GA"], args.project)
        elif command[0] == "ensure-iap-firewall":
            tag = "wf-iap-" + hashlib.sha256((args.project + "/" + args.zone + "/" + args.instance).encode()).hexdigest()[:16]
            existing = manage.resource(["compute", "firewall-rules", "describe", tag], args.project, optional=True)
            if existing is None:
                manage.gcloud(["compute", "firewall-rules", "create", tag, "--network", network, "--target-tags", tag,
                               "--allow=tcp:22", "--source-ranges=35.235.240.0/20"], args.project)
            elif existing.get("sourceRanges") != ["35.235.240.0/20"] or existing.get("targetTags") != [tag] or existing.get("allowed") != [{"IPProtocol": "tcp", "ports": ["22"]}]:
                raise ValueError("Existing IAP firewall rule differs from the intended SSH-only rule")
            manage.gcloud(["compute", "instances", "add-tags", args.instance, "--zone", args.zone, "--tags", tag], args.project)
        else:
            manage.gcloud(command, args.project)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True)
    parser.add_argument("--project-number", required=True)
    parser.add_argument("--repository-id", default="1437991235")
    parser.add_argument("--owner-id", required=True)
    parser.add_argument("--instance", required=True)
    parser.add_argument("--zone", default="us-west1-b")
    parser.add_argument("--region", default="us-west1")
    parser.add_argument("--runtime-account", required=True)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    try:
        commands, variables = plan(args)
        if args.apply:
            apply(args, commands)
        print(json.dumps({"commands": commands, "github_variables": variables}, indent=2))
    except (ValueError, OSError, KeyError, manage.DeploymentError):
        print("Bootstrap stopped; verify identities, existing VM configuration, API access, and permissions", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
