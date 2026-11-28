# Marketplace images

Recipes for one-click Huxley images on the DigitalOcean Marketplace, the Azure Marketplace and AWS Marketplace. Nothing here is published: building and listing need a release (published container images) and a publisher account with each marketplace. Both are the maintainers' decision.

## What's in an image

Ubuntu 24.04 LTS with:

- **Docker** and the [Compose deployment](../compose) in `/opt/huxley/deploy/compose`, pinned to the release (`HUXLEY_VERSION`), with its images already pulled. ClamAV's image is included too, so [malware scanning](../../docs/attachment-scanning.md) is one setting away.
- **A firewall** (ufw) that opens only SSH (rate-limited), HTTP and HTTPS. Automatic security updates are on.
- **Nothing secret.** On first boot, each server generates its own database password and key-encryption key (`first-boot.sh`, run once per instance by cloud-init). No two servers share them, and none are in the image.
- **Setup on first login.** The login message, and a reminder on root shells, say to run `huxley-setup`. It asks for the staff app's and the portal's domains, outgoing email, and the first user (the owner) and modules. Then it starts Huxley; Caddy obtains the certificates. It's safe to run again to change settings: the first user is only created once. Nothing listens on the web ports until it has run.

`cleanup.sh` then removes what the build left behind:

- SSH host keys and authorized keys;
- shell history and logs;
- apt caches;
- cloud-init state;
- the machine id.

It also zeroes free space. That's what DigitalOcean's `img_check.sh` and the other marketplaces' scans look for. Azure images are also generalized with `waagent -deprovision+user`.

## Building

You need [Packer](https://developer.hashicorp.com/packer) 1.11+ and a release whose images are published (`ghcr.io/usehuxley/huxley`, `web` and `portal` at the version's tag).

```sh
packer init deploy/marketplace
packer build -only='digitalocean.huxley' -var huxley_version=1.0.0 deploy/marketplace   # DIGITALOCEAN_TOKEN set
packer build -only='azure-arm.huxley'    -var huxley_version=1.0.0 deploy/marketplace   # az login; ARM_SUBSCRIPTION_ID set
packer build -only='amazon-ebs.huxley'   -var huxley_version=1.0.0 deploy/marketplace   # AWS credentials set
```

`-var prepull=false` leaves the images out: `huxley-setup` pulls them instead. That's for trying the recipe. A server still needs the release's images to be published when it's set up.

CI formats and validates the template for each cloud and lints the scripts. It also runs first boot and `huxley-setup` in a container with a stand-in Docker (`test/setup.sh`). That checks the secrets are made per server, the settings are written (root-only), Huxley is started, and the first user is created exactly once. An image hasn't been built in a real cloud yet.

## Listing

What each marketplace needs from the publisher. **This is the part that needs the maintainers:** accounts, agreements and a release.

**DigitalOcean Marketplace** (a 1-Click Droplet):
1. Join the Vendor Portal (marketplace.digitalocean.com/vendors) and accept its terms.
2. Build the snapshot. Run DigitalOcean's `img_check.sh` on a Droplet made from it; the cleanup here is meant to pass it.
3. Submit the snapshot with:
   - the listing: description, logo, screenshots;
   - "getting started" instructions (run `huxley-setup`);
   - a support URL and email, and a privacy policy URL.

   DigitalOcean reviews and tests it.

**Azure Marketplace** (a virtual machine offer):
1. Enroll in the Microsoft AI Cloud Partner Program and open a Partner Center account for the company.
2. Build the managed image and publish it to an Azure Compute Gallery.
3. In Partner Center, create a VM offer:
   - plans, pricing (free or bring-your-own-license) and the regions;
   - the listing, with a support contact and a privacy policy.
4. Microsoft certifies the image (a security scan and a test deployment) before it goes live.

**AWS Marketplace** (an AMI product):
1. Register as a seller in the AWS Marketplace Management Portal. Tax and banking details are needed only for paid listings.
2. Build the AMI and share it with the AWS Marketplace account.
3. Add it as an AMI product, then add the listing: description, usage instructions, support and the end-user licence.
4. AWS scans the AMI for vulnerabilities and malware before approving it.

For each, the listing should say plainly:
- Huxley is free and open source (the licence);
- the first login asks for the domains;
- DNS must point at the server first;
- the key-encryption key in `/opt/huxley/deploy/compose/secrets/kek` must be backed up ([Backup and restore](../../docs/backup.md)).
