Panic if merging two routers that each have a fallback
Currently when you `Router::merge` two routes that each have a fallback it'll pick the fallback of the router on the right hand side, and if only one has a fallback it'll pick that one. This might lead users to think that multiple fallbacks are supported (see https://github.com/tokio-rs/axum/discussions/480#discussioncomment-1601938), which it isn't.

I'm wondering if, in 0.4, we should change it such that merging two routers that each have a fallback causes a panic telling you that multiple fallbacks are not supported. This would also be consistent with the general routing which panics on overlapping routes.

The reason multiple fallbacks are not allowed is that `Router` tries hard to merge all your routes in one `matchit::Node` rather than nesting them somehow. This leads to significantly nicer internals and better performance.

@jplatte what do you think?
