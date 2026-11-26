#!/bin/sh
# role.sh: the IAM role of a build node. POSIX sh; runs where the node's owner has the AWS CLI and
# an identity that may write IAM and change the profile of an instance. Nothing here runs on the
# node, and provision.sh never calls AWS.
#
# A node has a role and an instance profile of its own, both named slot-node-<name>, and the
# role carries one customer-managed policy, slot-node-agent, and nothing else. The credentials of
# that role are what the metadata service hands out on the node, and a job that starts a
# container can reach it (README, docker). So the role allows only what the Systems Manager agent
# needs to stay a managed node and to take Run Command and Session Manager:
#
#   ssm:UpdateInstanceInformation   the agent's heartbeat, every 5 minutes; without it the
#                                   instance is no managed node
#   ssmmessages:*Channel (4)        the agent's channels to the service: Run Command documents
#                                   and Session Manager sessions arrive over them (agent
#                                   release 3.3.40 and later)
#   ec2messages:* (6)               the same delivery for an agent older than 3.3.40 and its
#                                   fallback; they name no resource of the account
#
# Left out of what AmazonSSMManagedInstanceCore allows: ssm:GetParameter and ssm:GetParameters
# (they read every parameter of the account), ssm:GetDocument and ssm:DescribeDocument (a Run
# Command document arrives in the message), and the State Manager, inventory, patch and
# distributor calls (associations, PutInventory, PutComplianceItems, GetManifest and the others):
# a node has no association. The agent logs a refused ListInstanceAssociations; Run Command and
# Session Manager do not need it.
#
# The instance's metadata service is set to tokens required and hop limit 1 (IMDSv2).
#
#   role.sh policy                  prints the policy document
#   role.sh trust                   prints the role's trust document
#   role.sh plan NAME INSTANCE      prints the aws commands of a first setup; calls nothing
#   role.sh check NAME INSTANCE     reads the account; one `drift:` line per difference, exit 1
#   role.sh apply NAME INSTANCE     makes what check names, then checks again. It names and
#                                   leaves an inline policy of the role and another role in the
#                                   profile (failed line, exit 1), and keeps the instance's
#                                   profile when a step before failed
#
# Not in the policy, because an account may not use them: kms:Decrypt for Session Manager
# sessions encrypted with a KMS key, and the S3 and CloudWatch Logs writes for session logs or
# Run Command output sent there. An account that has those set in its Session Manager
# preferences adds them to the document here.
#
# NAME is the node's name (NODE_NAME of its config), INSTANCE its instance id. AWS names another
# aws command (the tests use one).
set -u
AWS=${AWS:-aws}
POLICY=slot-node-agent

usage() {
  sed -n '2,/^set -u$/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2
  exit 2
}

policy() {
  cat <<'EOF'
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "AgentHeartbeat",
      "Effect": "Allow",
      "Action": "ssm:UpdateInstanceInformation",
      "Resource": "*"
    },
    {
      "Sid": "AgentChannels",
      "Effect": "Allow",
      "Action": [
        "ssmmessages:CreateControlChannel",
        "ssmmessages:CreateDataChannel",
        "ssmmessages:OpenControlChannel",
        "ssmmessages:OpenDataChannel"
      ],
      "Resource": "*"
    },
    {
      "Sid": "AgentChannelsOlderAgent",
      "Effect": "Allow",
      "Action": [
        "ec2messages:AcknowledgeMessage",
        "ec2messages:DeleteMessage",
        "ec2messages:FailMessage",
        "ec2messages:GetEndpoint",
        "ec2messages:GetMessages",
        "ec2messages:SendReply"
      ],
      "Resource": "*"
    }
  ]
}
EOF
}
trust() {
  cat <<'EOF'
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Principal": {
        "Service": "ec2.amazonaws.com"
      },
      "Action": "sts:AssumeRole"
    }
  ]
}
EOF
}
# A document on one line without blanks: no value in the two documents has one.
norm() { tr -d ' \t\n\r'; }
# `--output text` prints None for nothing.
some() { sed 's/^None$//' | tr '\t' '\n' | sed '/^$/d'; }

mode=${1:-}
case $mode in
  policy)
    policy
    exit 0
    ;;
  trust)
    trust
    exit 0
    ;;
  plan | check | apply) ;;
  *) usage ;;
esac
name=${2:-}
inst=${3:-}
[ -n "$name" ] && [ -n "$inst" ] && [ $# -eq 3 ] || usage
case $name in
  *[!A-Za-z0-9_.-]* | -* | ???????????????????????????????????????????????????????*)
    echo "role: NAME is letters, digits, '.', '_' and '-', at most 54 characters, not '$name'" >&2
    exit 2
    ;;
esac
case $inst in
  i-*[!0-9a-f]* | i- | i | [!i]* | i[!-]*)
    echo "role: INSTANCE is an instance id (i-...), not '$inst'" >&2
    exit 2
    ;;
esac
rn=slot-node-$name

if [ "$mode" = plan ]; then
  cat <<EOF
ACCOUNT=\$(aws sts get-caller-identity --query Account --output text)
aws iam create-policy --policy-name $POLICY --description 'What the Systems Manager agent of a slot build node needs, and nothing a job could use' --policy-document '$(policy | norm)'
aws iam create-role --role-name $rn --description 'Instance role of the slot build node $name' --assume-role-policy-document '$(trust | norm)'
aws iam attach-role-policy --role-name $rn --policy-arn "arn:aws:iam::\$ACCOUNT:policy/$POLICY"
aws iam create-instance-profile --instance-profile-name $rn
aws iam add-role-to-instance-profile --instance-profile-name $rn --role-name $rn
ASSOCIATION=\$(aws ec2 describe-iam-instance-profile-associations --filters Name=instance-id,Values=$inst Name=state,Values=associating,associated --query 'IamInstanceProfileAssociations[0].AssociationId' --output text)
aws ec2 replace-iam-instance-profile-association --association-id "\$ASSOCIATION" --iam-instance-profile Name=$rn
# An instance without a profile (ASSOCIATION is None) takes, in place of the line above: aws ec2 associate-iam-instance-profile --instance-id $inst --iam-instance-profile Name=$rn
aws ec2 modify-instance-metadata-options --instance-id $inst --http-tokens required --http-put-response-hop-limit 1
EOF
  exit 0
fi

ndrift=0
nfailed=0
drift() {
  echo "drift: role: $1"
  ndrift=$((ndrift + 1))
}
failed() {
  echo "failed: role: $1" >&2
  nfailed=$((nfailed + 1))
}
fixing() { [ "$mode" = apply ]; }
# A changing call: its name is printed, a failure is a failed line.
act() {
  if "$AWS" "$@" >/dev/null; then
    echo "changed: role: $1 $2"
  else
    failed "aws $1 $2"
    return 1
  fi
}

ident=$("$AWS" sts get-caller-identity --query Arn --output text) || {
  echo "role: aws sts get-caller-identity failed: no identity to work with" >&2
  exit 2
}
part=$(echo "$ident" | cut -d: -f2)
acct=$(echo "$ident" | cut -d: -f5)
parn=arn:$part:iam::$acct:policy/$POLICY
want_policy=$(policy | norm)
want_trust=$(trust | norm)
# The instance is read before anything is made: another region or account has no such instance.
mo=$("$AWS" ec2 describe-instances --instance-ids "$inst" --query 'Reservations[].Instances[].MetadataOptions.[HttpTokens,HttpPutResponseHopLimit]' --output text) || mo=
mo=$(echo "$mo" | some | tr '\n' ' ')
mo=${mo% }
[ -n "$mo" ] || {
  echo "role: aws ec2 describe-instances does not show $inst: the region or the account of this identity is another" >&2
  exit 2
}

# The policy and its default version.
ver=$("$AWS" iam get-policy --policy-arn "$parn" --query Policy.DefaultVersionId --output text 2>/dev/null | some)
if [ -z "$ver" ]; then
  drift "no policy $POLICY"
  fixing && act iam create-policy --policy-name "$POLICY" --description 'What the Systems Manager agent of a slot build node needs, and nothing a job could use' --policy-document "$want_policy"
else
  got=$("$AWS" iam get-policy-version --policy-arn "$parn" --version-id "$ver" --query PolicyVersion.Document --output json | norm)
  if [ "$got" != "$want_policy" ]; then
    drift "policy $POLICY version $ver is not the document of \`role.sh policy\`"
    fixing && act iam create-policy-version --policy-arn "$parn" --policy-document "$want_policy" --set-as-default
  fi
fi

# The role, who may assume it, and what is attached to it.
got=$("$AWS" iam get-role --role-name "$rn" --query Role.AssumeRolePolicyDocument --output json 2>/dev/null | norm)
have_role=1
if [ -z "$got" ]; then
  have_role=
  drift "no role $rn"
  fixing && act iam create-role --role-name "$rn" --description "Instance role of the slot build node $name" --assume-role-policy-document "$want_trust"
elif [ "$got" != "$want_trust" ]; then
  drift "role $rn is not assumed by ec2.amazonaws.com alone"
  fixing && act iam update-assume-role-policy --role-name "$rn" --policy-document "$want_trust"
fi
attached=$("$AWS" iam list-attached-role-policies --role-name "$rn" --query 'AttachedPolicies[].PolicyArn' --output text 2>/dev/null | some)
has=
others=
for a in $attached; do
  if [ "$a" = "$parn" ]; then has=1; else others="$others $a"; fi
done
# The node's policy goes on before another one comes off: a role the instance uses is at no
# moment without the agent's actions.
if [ -z "$has" ]; then
  drift "role $rn has no policy $POLICY attached"
  if fixing && [ "$nfailed" -eq 0 ]; then
    act iam attach-role-policy --role-name "$rn" --policy-arn "$parn" && has=1
  elif fixing; then
    failed "policy $POLICY is not attached to role $rn: a step before failed"
  fi
fi
for a in $others; do
  drift "role $rn has another policy attached: $a"
  fixing || continue
  if [ -n "$has" ] && [ "$nfailed" -eq 0 ]; then
    act iam detach-role-policy --role-name "$rn" --policy-arn "$a"
  else
    failed "policy $a stays on role $rn: $POLICY is not attached, or a step before failed"
  fi
done
# An inline policy is not removed here: it has no copy elsewhere. A list that cannot be read is
# not an empty list.
if [ -n "$have_role" ]; then
  if inline=$("$AWS" iam list-role-policies --role-name "$rn" --query PolicyNames --output text 2>/dev/null); then
    for p in $(echo "$inline" | some); do
      drift "role $rn has an inline policy: $p"
      fixing && failed "inline policy $p of role $rn stays: read it, then aws iam delete-role-policy --role-name $rn --policy-name $p"
    done
  else
    drift "the inline policies of role $rn could not be read"
    fixing && failed "aws iam list-role-policies --role-name $rn"
  fi
fi

# The instance profile holds this role and no other.
if "$AWS" iam get-instance-profile --instance-profile-name "$rn" --query InstanceProfile.InstanceProfileName --output text >/dev/null 2>&1; then
  roles=$("$AWS" iam get-instance-profile --instance-profile-name "$rn" --query 'InstanceProfile.Roles[].RoleName' --output text | some)
else
  roles=
  drift "no instance profile $rn"
  fixing && act iam create-instance-profile --instance-profile-name "$rn"
fi
case $roles in
  "$rn") ;;
  '')
    drift "instance profile $rn holds no role"
    fixing && act iam add-role-to-instance-profile --instance-profile-name "$rn" --role-name "$rn"
    ;;
  *)
    drift "instance profile $rn holds the role $roles"
    fixing && failed "instance profile $rn holds another role: aws iam remove-role-from-instance-profile --instance-profile-name $rn --role-name $roles"
    ;;
esac

# The instance uses that profile. A profile made a moment ago is not known to EC2 at once, and
# an association made a moment ago is `associating` before it is `associated`: both states count.
assoc=$("$AWS" ec2 describe-iam-instance-profile-associations --filters "Name=instance-id,Values=$inst" Name=state,Values=associating,associated --query 'IamInstanceProfileAssociations[].[AssociationId,IamInstanceProfile.Arn]' --output text | some | tr '\n' ' ')
aid=
cur=
set -- $assoc
while [ $# -ge 2 ]; do
  if [ -z "$aid" ] || [ "${2##*/}" = "$rn" ]; then
    aid=$1
    cur=${2##*/}
  fi
  shift 2
done
if [ "$cur" != "$rn" ]; then
  drift "instance $inst uses the instance profile '${cur:-none}', not $rn"
  if fixing && [ "$nfailed" -eq 0 ]; then
    err=$(mktemp)
    n=0
    while :; do
      if [ -n "$aid" ]; then
        "$AWS" ec2 replace-iam-instance-profile-association --association-id "$aid" --iam-instance-profile "Name=$rn" >/dev/null 2>"$err" && break
      else
        "$AWS" ec2 associate-iam-instance-profile --instance-id "$inst" --iam-instance-profile "Name=$rn" >/dev/null 2>"$err" && break
      fi
      n=$((n + 1))
      if [ "$n" -ge "${ROLE_TRIES:-12}" ]; then
        failed "instance $inst did not take the instance profile $rn: $(tail -1 "$err")"
        break
      fi
      sleep "${ROLE_WAIT:-5}"
    done
    rm -f "$err"
    if [ "$n" -lt "${ROLE_TRIES:-12}" ]; then
      echo "changed: role: instance $inst uses the instance profile $rn (before: '${cur:-none}')"
      echo "note: credentials the old role gave out stay valid until they expire. The agent takes the new role at its next refresh; \`systemctl restart amazon-ssm-agent\` on the node does it now."
    fi
  elif fixing; then
    failed "instance $inst keeps its profile: a step before failed"
  fi
fi

# The metadata service wants a token (IMDSv2) and its answer goes one hop: a plain GET, which is
# all a URL fetch of the docker daemon can send, gets no credential, and neither does a
# container behind a bridge.
if [ "$mo" != 'required 1' ]; then
  drift "instance $inst: the metadata service has tokens and hop limit '$mo', not 'required 1'"
  fixing && act ec2 modify-instance-metadata-options --instance-id "$inst" --http-tokens required --http-put-response-hop-limit 1
fi

if fixing; then
  [ "$nfailed" -eq 0 ] || exit 1
  [ "$ndrift" -eq 0 ] || exec sh "$0" check "$name" "$inst"
fi
echo "$ndrift drift"
[ "$ndrift" -eq 0 ]
