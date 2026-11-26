# Reaching the build node without the workstation's address — design

Status: proposal, 2026-11-25, against slot 0.38.0. Nothing here is built and nothing in a cloud
account was changed for it. The choice is the node owner's: every option changes the cloud
account, and some change who holds which credential.

Terms as in `gitlab-runner.md`: the **workstation** runs the local daemon and the agent sessions;
the **node** is the cloud build machine of `contrib/node`. *(unverified)* marks a claim not checked
against a document or a run.

## 1. What failed

The node takes ssh on port 22 from one address: a security-group rule that allows the
workstation's public address as a `/32`. On 2026-11-25 the workstation's egress address changed
(its access network assigns it), the rule still named the old one, and every `ssh` from the
workstation timed out from about 06:13 CET. All remote jobs stopped until the rule was edited in
the account, which only the owner may do.

Every job crosses on the same ssh path: `rsync` of the tree and the `slot shell` run use the build
key (it may only run `slot shell`), and a login that sets the node up by hand uses the setup key,
let in only after `provision.sh apply --open`. Nothing is copied to a node ahead of jobs: it
installs everything itself (`provision.sh apply`). The options below change how that ssh connection reaches the node;
the keys, the forced command and the deny lists stay as they are.

## 2. Options

### A. ssh through Session Manager (recommended)

The ssh client's `ProxyCommand` opens the connection through AWS Systems Manager instead of the
node's port 22:

```
Host <instance-id>
  ProxyCommand sh -c "aws ssm start-session --target %h --document-name AWS-StartSSHSession --parameters 'portNumber=%p'"
```

- Inbound: the security group allows no inbound port at all. "You can close inbound ports on the
  node" ([Session Manager, SSH](https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-getting-started-enable-ssh-connections.html)).
- The node already runs the SSM agent with an instance profile: root bundles reach it through
  SSM Run Command. The page asks for SSM Agent 2.3.672.0 or later; the node's version is not read
  here *(unverified)*.
- The workstation needs the AWS CLI with the Session Manager plugin 1.1.23.0 or later, and an AWS
  identity allowed `ssm:StartSession` on this one instance with the document
  `AWS-StartSSHSession`, and nothing else. That identity is a credential on the workstation, not on
  the node; scoping it is the owner's decision.
- Cost: "No additional charges for usage on Amazon EC2 instances"
  ([Systems Manager pricing](https://aws.amazon.com/systems-manager/pricing/)).
- Operation: slot's own `SLOT_SSH` and the setup host in the ssh config take the `ProxyCommand`;
  slot's `ControlMaster` keeps one session open for many jobs, so the session start is paid once
  per `ControlPersist` period *(unverified: start time not measured)*. Session Manager does not log
  ssh sessions: "Logging isn't available for Session Manager sessions that connect through port
  forwarding or SSH" (same page). A short-lived credential that expires ends new connections; a
  long-lived one is a secret to rotate.

### B. EC2 Instance Connect Endpoint

A private endpoint in the node's VPC tunnels the ssh connection; the security group allows port
22 only from the endpoint.

- Cost: "There is no additional cost for using EC2 Instance Connect Endpoints", except data
  transfer across Availability Zones
  ([Connect using EC2 Instance Connect Endpoint](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/connect-with-ec2-instance-connect-endpoint.html)).
- Limits that matter for slot: one connection lasts at most 1 hour; an endpoint carries at most 20
  concurrent connections, and a VPC has one endpoint (both not adjustable,
  [quotas](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/eice-quotas.html)). "High volume
  data transfers are throttled" (connect page): the first sync of a tree can send GB (3.3 GB
  measured for one tree without its build output).
- The workstation needs an identity allowed `ec2-instance-connect:OpenTunnel` for the endpoint.
- Compared with A: a new resource in the VPC, the 1-hour cap and the throttle, for nothing A
  lacks here.

### C. Update the rule when the address changes

A timer on the workstation reads its egress address and, when it changed, replaces the `/32` in
the security-group rule.

- Keeps today's path and the 1-address exposure.
- The workstation holds a credential that may write that security group
  (`ec2:AuthorizeSecurityGroupIngress`, `ec2:RevokeSecurityGroupIngress` on one group). A
  credential that writes firewall rules is a larger grant than A's `StartSession`.
- Jobs fail between the change and the next timer run; reading the egress address needs an
  outside service.

### D. Allow the access network's range

Replace the `/32` with the provider's prefix. Nothing to operate, but the port then answers
every customer of that provider, and the provider can move the workstation outside the prefix
*(unverified: the range of the access network is not known here)*.

### E. Port 22 open to every address

Key-only login, the build key forced to `slot shell`, password login off. Nothing to operate;
sshd faces the internet's scanners, and a defect in sshd or in `slot shell` is reachable by
anyone.

## 3. Recommendation

A. It removes the inbound port instead of moving it, uses an agent and an instance profile the
node already has, costs nothing on EC2, and the new grant is the narrowest of A, B and C.

What A needs, all the owner's: the scoped AWS identity for the workstation, the Session Manager
plugin on the workstation, and the removal of the port-22 rule after A works. In slot: the ssh
config of the node gains the `ProxyCommand` (private config, no code change); `contrib/node`'s
README names it, and `provision.sh check` reports an inbound port-22 rule as drift only if the
owner wants that *(not built)*.

## 4. Independent of the choice

When the node cannot be reached, a job no longer runs on the workstation instead: from the next
release a remote job that cannot start exits 3 with one line (`client.rs`), and the old fallback
needs an explicit variable.
