# lama
`lama` is short for "lab manager". It is a tool for automatically deploying virtual labs. You provide it the path to an exported lab and it deploys the lab to your local virtualization environment (such as Hyper-V). If the lab is located on a remote machine, it will be automatically downloaded to your local machine first.

Currently only Hyper-V is supported. Support for other virtualization environments may come later.

# How to Use
To deploy an exported lab run the following commandline:
```
lama deploy <path to an exported lab>
```
