# RustFS Manager Operator

## Getting started

This operator is supposed to connect to an existing **RustFS** instance and manage bucket and users on it.

### How to install

The operator is distributed as a helm chart, that can be installed either from **gchr** as an OCI artifact.


```shell
helm install rustfs-manager-operator oci://ghcr.io/badhouseplants.net/rustfs-manager-operator/rustfs-manager-operator --version 0.1.2
```

#### Docs

Documentation is added as a dependency to the main helm chart, you can install it by setting `.docs.enabled` value to `true`.  Then you will have the up-to-date documentation deployed to your cluster as well.

### Connect the operator to RustFS

Once the operator is running, you will need to create a `RustFSIntsance` CR to connect the operator to your **RustFS**. You can either create it manually, or use a helm chart as well.

#### Helm chart

1. Create values.yaml

```yaml
# values.yaml
endpoint: https://your.rust.fs
username: admin
password: qwertyu9
```

```shell
helm install <your instance name> oci://ghcr.io/badhouseplants.net/rustfs-manager-operator/rustfs-instance --version 0.1.2 -f values.yaml
```

And wait until it becomes ready.

```shell
kubectl get rustfs <your instance name>
NAME                   ENDPOINT              REGION      TOTAL BUCKETS   STATUS
<your instance name>   <your instance url>   us-east-1   2               true
```


### Start using

Now you can start creating buckets and users.

#### Create a bucket

```yaml
apiVersion: rustfs.badhouseplants.net/v1beta1
kind: RustFSBucket
metadata:
  name: <bucket name>
  namespace: <application namespace>
spec:
  # When cleanup is set to true, bucket will be removed from the instance
  cleanup: false
  # On which instance this bucket should be created
  instance: rustfs-instance
  # If true, bucket will be created with object locking
  objectLock: false
  # If true, bucket will be created with versioning
  versioning: false
```

```shell
kubectl get bucket <bucket name>
NAME            BUCKET NAME                ENDPOINT     REGION      STATUS
<bucket-name>   <namespace>-<bucket-name>   <endpoint>   us-east-1   true
```

When bucket is created, there will be a secret created in the same namespace: `<bucket name>-bucket-info`

```shell
kubectl get configmap <bucket name>-bucket-info -o yaml

apiVersion: v1
kind: ConfigMap
data:
  AWS_BUCKET_NAME: <bucket name>
  AWS_ENDPOINT_URL: <endpoint>
  AWS_REGION: <region>
```

#### Creating a user

When the bucket is ready, you can create a user that will have access to this bucket:

```yaml
apiVersion: rustfs.badhouseplants.net/v1beta1
kind: RustFSBucketUser
metadata:
  name: <username>
  namespace: <application namespace>
spec:
  bucket: <a name of the bucket CR>
  # User will be removed from the RustFS instance if set to true
  cleanup: false
  access: readWrite # or readOnly
```


```shell
kubectl get bucketuser <username>
NAME            USER NAME                   SECRET                       CONFIGMAP                       ACCESS      STATUS
<username>      <namespace>-<username>      <username>-bucket-creds      <bucket name> -bucket-info      readWrite   true
```

Operator will also add a Secret to the same namespace: `<username>-bucket-creds`, that will contain the following keys:

- AWS_ACCESS_KEY_ID
- AWS_SECRET_ACCESS_KEY

You can use them to connect your application to the bucket.
