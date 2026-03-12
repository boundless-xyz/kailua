# Design Overview

Kailua's ZK fault proof game operates seamlessly in two main ways:
1. Disputes are non-interactive, removing the need for a "bisection" or multi-round on-chain search for a fault.
2. Disputes are implicit between contradictory proposals, removing the need for a special "challenge" transaction.

## Sequencing

Sequencing proposals in Kailua can utilize the publication of extra data instead of only the single commitment submitted in other protocols
in order to reduce the amount of proving work required to resolve disputes.
This extra data consists of commitments to the intermediate sequencing states, which allows the generated ZK proofs to
target only a sub-sequence of blocks comprising one transition between the intermediate states instead of the entire
proposal.

```admonish note
While Kailua can be configured to operate using a single published commitment per proposal, this may make the proving
work required to resolve disputes expensive for chains with very low block times, or a significantly large number
of blocks per proposal in general.
```

<div style="text-align: center;">

```mermaid
---
title: Example Kailua Sequencing Proposal
---
graph LR;
  A -.8.-> B((B)) -.8.-> C((C)) -.8.-> D((D)) -.8.-> E((E)) -.8.-> F((F)) -.8.-> G((G)) -.8.-> H((H)) -.8.-> I;
```

```mermaid
---
title: Example Standard Sequencing Proposal
---
graph LR;
  A --64--> I;
```

</div>

The above two diagrams illustrate the extended data in Kailua sequencing proposals.
While a standard proposal for sequencing 64-blocks would only comprise a single commitment, the Kailua variant here is
configured to also require the commitment for every 8th block.
In this configuration, any Kailua fault proof would only have to provably derive a sequence of at most 8 blocks. 

[//]: # (```admonish note)

[//]: # (To save on DA costs, blobs or alternative DA layers can be used to publish intermediate commitments.)

[//]: # (```)

## Disputes

Each new sequencing proposal implicitly disputes the last existing proposal that contradicts it.
Once this happens, a proof is required to demonstrate which of the two contradictory proposals, if any, commits to the
correct sequencing state at their first point of divergence.
The proof then eliminates one, or both, contradictory proposals, and neither proposals can be finalized until the proof
is submitted.

```admonish note
While any new contradictory proposal has to be made within the timeout period of the prior proposal it contradicts, 
proofs are granted an unlimited amount of time for permissionless submission by anyone.
```

<div style="text-align: center;">

```mermaid
---
title: Disputes Example
---
graph LR;
    A --> B;
    A --✓--> B';
    A --✓--> B'';
    
    B --> C';
    B --✓--> C;
    
    B' --> C'';
    C --> D;

    D --> E;    
    D --✗--> E';
```

</div>

Consider the above example scenario, where proposal `A` is finalized, while `B`, `C`, `D` and `E` are the only correct sequencing
proposals pending finalization, while all others are invalid.

A plain edge from a parent to a child indicates that the child proposal was made while no contradictory siblings should have existed.
A checkmark on the edge indicates that the proposal was made within the timeout period of the contradicotry sibling.
A crossmark indicates that the timeout period of the contradictory sibling proposal had expired before the child proposal was introduced.

The following three challenges are the only ones implied:
1. `B'` challenges `B`
2. `B''` challenges `B` (the proof for the prior challenge will eliminate `B'`).
3. `C` challenges `C'`.

The following two invalid proposals created no challenges:
1. `C''` has no siblings and therefore causes no implicit challenges, but will be eliminated once its parent `B'` is eliminated.
2. `E'` was made after the timeout period for `E` had expired, and was automatically eliminated.

In this scenario, `B` can only be finalized once two proofs are submitted to resolve its disputes against `B'` and `B''`.
Proposal `C` can only be finalized once a proof resolves its dispute against `C'`, and its parent `B` is finalized.
`D` has no contenders and can be finalized once its parent `C` is finalized.
The timeout period for `E` had passed before `E'` was introduced, and therefore `E` can be finalized once its parent `D` is finalized.

## Fault Proving Permits

Kailua includes an optional permit system that allows provers to acquire exclusive rights to submit fault proofs for
disputed proposals.
A prover can lock collateral to gain an exclusive window during which only their proof submission earns the fault proof
reward.

```admonish note
Permits are entirely optional. Fault proofs can be submitted without acquiring a permit, and the dispute will still
be resolved. However, permit holders may receive preferential payouts.
```

### Permit Lifecycle

Each permit goes through three phases determined by two time durations configured at contract deployment:
a delay duration and a total permit duration.

<div style="text-align: center;">

```mermaid
---
title: Permit Lifecycle
---
graph LR
    A[Acquired] -- delay elapsed --> B[Active]
    B -- duration elapsed --> C[Expired]
```

</div>

| Phase       | Description                                    | Guaranteed Reward? |
|-------------|------------------------------------------------|--------------------|
| **Delayed** | Permit has been acquired but is not yet active | No                 |
| **Active**  | Permit is live and grants exclusive rights     | Yes                |
| **Expired** | Permit has lapsed                              | No                 |

### Collateral

Acquiring a permit requires locking collateral proportional to the elimination reward.
The collateral is returned when the permit is released after a proof is submitted.
Active permit holders also receive a share of collateral from expired permits.

The number of permits that can be issued for a given dispute is bounded: new permits can only be acquired at a rate
that grows exponentially with the number of expired permits.
This prevents any single party from monopolizing permits indefinitely while still allowing competitive acquisition.

### Reward Distribution

When a fault proof is submitted, the reward recipient for the proposer's collateral is determined as follows:

1. **Exactly one permit exists and is active at proof time**: The permit holder receives the fault proof reward
   instead of the prover.
2. **Otherwise**: The prover who submitted the fault proof receives the reward as usual.

When a permit holder releases their permit, the collateral payout depends on the state of all permits at proof time:

1. **Active at proof time**: The holder receives their collateral back, plus an equal share of any expired permit
   collateral.
2. **Expired before proof time**: The holder's collateral is forfeited to the pool split among active holders.

```admonish warning
If a permit expires before the fault proof is submitted, the permit holder forfeits their collateral to the pool
of active permit holders.
```