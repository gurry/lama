# lama
`lama` is short for "lab manager". It is a commandline tool for automatically deploying and managing virtual labs. 

# Capabilities
When fully developed, `lama` will be able to:
1. _Deploy_ an exported lab, which means to create the lab from scratch on your physical machine. It will include automatically creating all the network switches and connecting them together as specified. If the exported lab resids on a remote machine, it will be automatically downloaded first.
2. _Drop_ the lab, which means deleting all its VMs, with the option of retaining the files of the deleted VMs on the disk so that it can be deployed again later.
3. _Provision_ the lab, which means configuring each VM in the lab with some script such as PowerShell after it has been deployed. This could be useful for example to install certain software or configure some settings in the OS before you use the VMs. You should be able to run provisioning both when you first deploy a lab and also multiple times later on a lab that's already there.
4. _Export_ a lab, so that others can later deploy it.

The current version is more at a PoC stage. A very primitive form of _deploy_ and _drop_ functionality has been implemented. You can try it out, but don't use in production.

# Supported Environments
Currently only Hyper-V is supported. Support for other virtualization environments may come later.

# How to Use
The following is the syntax of the various commands.

To deploy an exported lab:
```
lama deploy <path to the exported lab>
```
To drop a deployed lab:
```
lama drop <path to the lab on the local disk>
```
To provision a deployed lab:
```
lama provision <path to the lab on the local disk> --ps <path to powershell script to run>
```
To export a deployed lab:
```
lama export <path to the lab on the local disk> <path where the exported lab should be placed>
```
