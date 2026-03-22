# RustFS Manager Operator

An operator to manage bucket and user on a RustfFS instance through Kubernetes CRDs.

## Getting started

Find better docs here: <https://rustfs.badhouseplants.net>


### Install the operator

```shell
helm install rustfs-manager-operator oci://gitea.badhouseplants.net/badhouseplants/rustfs-manager-operator/rustfs-manager-operator --version 0.1.0
```
### Connect it to a RustFS instance

1. Create a values file:

```yaml
# values.yaml
endpoint: https://your.rust.fs
username: admin
password: qwertyu9
```

2. Install the **rustfs-instance** helm chart

```shell
helm install rustfs-instance oci://gitea.badhouseplants.net/badhouseplants/rustfs-u/rustfs-instance --version 0.1.0 -f ./values.yaml
```

### Start creating Buckets and Users

#### Buckets

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

#### Users

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

### Access credentials via ConfigMaps and Secrets

#### ConfigMap:

```shell
kubectl get configmap <bucket name>-bucket-info -o yaml

apiVersion: v1
kind: ConfigMap
data:
  AWS_BUCKET_NAME: <bucket name>
  AWS_ENDPOINT_URL: <endpoint>
  AWS_REGION: <region>
```

#### Secret:

```shell
kubectl get secret <username>-bucket-creds -o yaml

apiVersion: v1
kind: ConfigMap
data:
  AWS_ACCESS_KEY_ID: <username>
  AWS_SECRET_ACCESS_KEY: <a generated password>
```
