CannotBorrowOwnedData on decode_from_slice for some types that implement Serialize/Deserialize - bincode::serde
Thanks for the excellent work on bincode. I'm currently testing a simple memory based cache for a system I'm writing. In various places I'm calling `serde::encode_to_vec` and `serde::decode_from_slice`. I'm running into an error on `decode_from_slice` indicating `CannotBorrowOwnedData`. I briefly looked at the error, but haven't spent much time digging into it yet. 

I can do something simple like the following:

```rust
#[derive(Serialize, Deserialize)]
pub struct MyStruct {
    name: String,
}

```

and then 

```rust
  let test = MyStruct {
    name: "Test Value".to_owned(),
  };

  let value = MemCache::set_data::<MyStruct>(&cache_id, &test, 5).await?;
  let model = MemCache::get_data::<MyStruct>(&cache_id).await?;
```

where `set_data`

```rust
    async fn set_data<T>(key: &Uuid, cache_data: &T, expire_seconds: i64) -> AsyncResult<bool>
    where
        T: Send + Sync + Serialize,
    {
        let config = Configuration::standard();
        let mut guard = MEM_CACHE.write().await;

        let encoded = bincode::serde::encode_to_vec(&cache_data, config)?;
        let cache_item = CacheItem::new(encoded, expire_seconds)?;

        guard.cache_items.insert(key.clone(), cache_item);
        Ok(true)
    }
```

and `get_data`

```rust
async fn get_data<T>(key: &Uuid) -> AsyncResult<T>
    where
        T: Send + Sync + DeserializeOwned,
    {
        let config = Configuration::standard();
        let guard = MEM_CACHE.read().await;
        /** simplified example ***/
        let cache_item = match guard.cache_items.get(key).unwrap();
        let (decoded, _len): (T, usize) =
            bincode::serde::decode_from_slice(&cache_item.data[..], config)?;
        Ok(decoded)
    }
````

however, if I provide a struct that has other  types, E.G. "Uuid and DateTime<Utc>(chrono)", both of which have Serialize/Deserialize traits implement by their respective crates, I can successfully call `encode_to_vec` but `decode_from_slice` returns the aforementioned error. 

E.G.: 

```rust
#[derive(Serialize, Deserialize)]
pub struct CustomerTest {
    pub id: Option<Uuid>,
    pub email_address: Option<String>,
    pub is_active: Option<bool>,
    pub date_stamp: Option<DateTime<Utc>>,
}
```

```rust
    let test2 = CustomerTest {
        id: Some(Uuid::new_v4()),
        email_address: Some("test@test_domain.io".to_owned()),
        is_active: Some(true),
        date_stamp: Some(Utc::now()),
    };

    let cache_id = Uuid::new_v4();
    let value = MemCache::set_data::<CustomerTest>(&cache_id, &test2, 5).await?;
    let model = MemCache::get_data::<CustomerTest>(&cache_id).await?;
```

Testing it further, it appears that removing the `Uuid` type along with the `DateTime<Utc>` type results in a successful call to `decode_from_slice`. Again, it's possible to call `encode_to_vec` with all of the types listed above, but it does not appear to currently be possible to decode without raising the `CannotBorrowOwnedData` error. 

Any advice on this would be welcomed. Thanks again for your efforts on bincode.
