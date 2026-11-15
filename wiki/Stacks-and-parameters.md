A stack is one document that names the networks, subnets, machines, routers, pods, listeners and DNS records a service needs. `pi stack apply` writes those rows; a second apply updates the rows the stack made and deletes the ones the document no longer names.

A **parameter** is a value the document leaves open. You declare it in the stack spec and give its value when you create or update the stack, so the same document serves more than one shape of service.

## Declaring a parameter

```yaml
name: web
spec:
  name: web
  document: |
    kind: network
    name: "{{ param.size }}-net"
    spec:
      mtu: "{{ param.mtu }}"
  declared:
  - name: size
    default: small
    description: how the network is named
  - name: mtu
    default: "1450"
  parameters:
    size: big
```

`spec.declared` lists the parameters the document may name: a `name`, an optional `default`, and an optional `description` for whoever reads the stack. `spec.parameters` holds the values given for them.

## Writing a reference

A reference is written `{{ param.NAME }}` and **has to be quoted** — `mtu: "{{ param.mtu }}"`. Bare `{{` is the start of a mapping to YAML, so an unquoted reference is not the value you meant.

Substitution happens in the parsed document, so the quotes do not reach the row. A reference that stands alone becomes the value itself, typed as its value reads:

| written | with | the row gets |
| --- | --- | --- |
| `mtu: "{{ param.mtu }}"` | `mtu=1500` | the number `1500` |
| `dhcp: "{{ param.on }}"` | `on=true` | the flag `true` |
| `name: "{{ param.size }}-net"` | `size=big` | the text `big-net` |

A reference set inside other text is replaced within it, and several in one string are joined into that text. The name a resource is created under follows the same rule, so a name built from a parameter is a plain name.

Text that is not a reference is left exactly as it is written. `{{ jq.field }}` or a stray `{{` belong to whatever wrote them and are never touched.

A parameter goes where a value belongs. A resource's `name` may be built from one; its `kind`, and any key, may not — `kind: "{{ param.k }}"` is refused where it sits.

## What is refused

An apply stops before anything is created, naming what is wrong, when:

- a value was given for a parameter nothing declared — `no parameter is declared for 'colour'`
- the document names a parameter that is not declared — `the document names undeclared parameter 'gone'`
- a declared parameter has neither a value nor a default — `no value given for parameter 'mtu' and it has no default`
- a declared parameter is never named, or the same one is declared twice

Create and update refuse the first, third and fourth of these; a reference to something undeclared is caught at apply, when the document is read.

## Values and revisions

A stack keeps the document as you wrote it, references and all, and keeps the parameter values with each revision in `status.history`. A rollback puts an earlier revision's spec back — document, declared parameters and values together — and applies it, so the network that was named `small-net` comes back named `small-net`.

An update that changes one parameter value re-applies the document. Rows whose spec the value feeds are updated in place. A change to a resource's name creates the new row and deletes the old one, because a row is found by its name.

## Kits and workflows

A kit takes the same `declared` and `parameters` and is applied the same way, without keeping history. A workflow is an ordered list of stack names; each stack is applied with the values it holds.
