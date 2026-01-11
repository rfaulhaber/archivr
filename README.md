# archivr

A Tumblr backup tool.


## Prerequisites

### Register a new Tumblr application

In order to interact with the Tumblr API, archivr needs an OAuth consumer key and secret.

1. In Tumblr, go to Settings > Apps
2. Click on the "Register" link at the bottom
3. Click the green "Register application" button
4. Fill out the following fields:
   1. **Application name**: archivr (this doesn't really matter, this is for your reference)
   2. **Application website**: codeberg.org/ryf/archivr (again, doesn't really matter)
   3. **Application description**: Tumblr backup tool
   4. **Administrative contact email**: your email
   5. **Default callback URL**: `https://localhost:6263/callback`
   6. **OAuth2 redirect URLs**: `https://localhost:6263/redirect`
5. Click "Save changes"

## Usage

```sh
archivr <BLOG_NAME> --consumer-key <YOUR CONSUMER KEY> --consumer-secret <YOUR CONSUMER SECRET>
```

This will kick off a job to back up an entire blog.

### Job config file

You can specify all of the CLI arguments in a config file as well, passing `--config-file <PATH>` instead.
