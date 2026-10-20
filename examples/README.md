# Examples

`door-bundle/` is a two-concept OKF bundle. Try:

```sh
cp -r examples/door-bundle /tmp/door && cd /tmp/door
wisp init && WISP_ACTOR=human:you wisp record -m "import"
wisp cat tables/door.md --json | jq '.data.sections[] | {heading, section, version}'
```

Then follow the five-minute tour in the top-level README.
