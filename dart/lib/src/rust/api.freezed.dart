// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'api.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$Backend {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Backend);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'Backend()';
}


}

/// @nodoc
class $BackendCopyWith<$Res>  {
$BackendCopyWith(Backend _, $Res Function(Backend) __);
}


/// Adds pattern-matching-related methods to [Backend].
extension BackendPatterns on Backend {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( Backend_Local value)?  local,TResult Function( Backend_Remote value)?  remote,required TResult orElse(),}){
final _that = this;
switch (_that) {
case Backend_Local() when local != null:
return local(_that);case Backend_Remote() when remote != null:
return remote(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( Backend_Local value)  local,required TResult Function( Backend_Remote value)  remote,}){
final _that = this;
switch (_that) {
case Backend_Local():
return local(_that);case Backend_Remote():
return remote(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( Backend_Local value)?  local,TResult? Function( Backend_Remote value)?  remote,}){
final _that = this;
switch (_that) {
case Backend_Local() when local != null:
return local(_that);case Backend_Remote() when remote != null:
return remote(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  local,TResult Function( PeerAddr peer)?  remote,required TResult orElse(),}) {final _that = this;
switch (_that) {
case Backend_Local() when local != null:
return local();case Backend_Remote() when remote != null:
return remote(_that.peer);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  local,required TResult Function( PeerAddr peer)  remote,}) {final _that = this;
switch (_that) {
case Backend_Local():
return local();case Backend_Remote():
return remote(_that.peer);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  local,TResult? Function( PeerAddr peer)?  remote,}) {final _that = this;
switch (_that) {
case Backend_Local() when local != null:
return local();case Backend_Remote() when remote != null:
return remote(_that.peer);case _:
  return null;

}
}

}

/// @nodoc


class Backend_Local extends Backend {
  const Backend_Local(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Backend_Local);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'Backend.local()';
}


}




/// @nodoc


class Backend_Remote extends Backend {
  const Backend_Remote({required this.peer}): super._();
  

 final  PeerAddr peer;

/// Create a copy of Backend
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Backend_RemoteCopyWith<Backend_Remote> get copyWith => _$Backend_RemoteCopyWithImpl<Backend_Remote>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Backend_Remote&&(identical(other.peer, peer) || other.peer == peer));
}


@override
int get hashCode => Object.hash(runtimeType,peer);

@override
String toString() {
  return 'Backend.remote(peer: $peer)';
}


}

/// @nodoc
abstract mixin class $Backend_RemoteCopyWith<$Res> implements $BackendCopyWith<$Res> {
  factory $Backend_RemoteCopyWith(Backend_Remote value, $Res Function(Backend_Remote) _then) = _$Backend_RemoteCopyWithImpl;
@useResult
$Res call({
 PeerAddr peer
});




}
/// @nodoc
class _$Backend_RemoteCopyWithImpl<$Res>
    implements $Backend_RemoteCopyWith<$Res> {
  _$Backend_RemoteCopyWithImpl(this._self, this._then);

  final Backend_Remote _self;
  final $Res Function(Backend_Remote) _then;

/// Create a copy of Backend
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? peer = null,}) {
  return _then(Backend_Remote(
peer: null == peer ? _self.peer : peer // ignore: cast_nullable_to_non_nullable
as PeerAddr,
  ));
}


}

/// @nodoc
mixin _$LinkRoute {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is LinkRoute);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'LinkRoute()';
}


}

/// @nodoc
class $LinkRouteCopyWith<$Res>  {
$LinkRouteCopyWith(LinkRoute _, $Res Function(LinkRoute) __);
}


/// Adds pattern-matching-related methods to [LinkRoute].
extension LinkRoutePatterns on LinkRoute {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( LinkRoute_Direct value)?  direct,TResult Function( LinkRoute_Relay value)?  relay,required TResult orElse(),}){
final _that = this;
switch (_that) {
case LinkRoute_Direct() when direct != null:
return direct(_that);case LinkRoute_Relay() when relay != null:
return relay(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( LinkRoute_Direct value)  direct,required TResult Function( LinkRoute_Relay value)  relay,}){
final _that = this;
switch (_that) {
case LinkRoute_Direct():
return direct(_that);case LinkRoute_Relay():
return relay(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( LinkRoute_Direct value)?  direct,TResult? Function( LinkRoute_Relay value)?  relay,}){
final _that = this;
switch (_that) {
case LinkRoute_Direct() when direct != null:
return direct(_that);case LinkRoute_Relay() when relay != null:
return relay(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String addr)?  direct,TResult Function( String url)?  relay,required TResult orElse(),}) {final _that = this;
switch (_that) {
case LinkRoute_Direct() when direct != null:
return direct(_that.addr);case LinkRoute_Relay() when relay != null:
return relay(_that.url);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String addr)  direct,required TResult Function( String url)  relay,}) {final _that = this;
switch (_that) {
case LinkRoute_Direct():
return direct(_that.addr);case LinkRoute_Relay():
return relay(_that.url);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String addr)?  direct,TResult? Function( String url)?  relay,}) {final _that = this;
switch (_that) {
case LinkRoute_Direct() when direct != null:
return direct(_that.addr);case LinkRoute_Relay() when relay != null:
return relay(_that.url);case _:
  return null;

}
}

}

/// @nodoc


class LinkRoute_Direct extends LinkRoute {
  const LinkRoute_Direct({required this.addr}): super._();
  

 final  String addr;

/// Create a copy of LinkRoute
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$LinkRoute_DirectCopyWith<LinkRoute_Direct> get copyWith => _$LinkRoute_DirectCopyWithImpl<LinkRoute_Direct>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is LinkRoute_Direct&&(identical(other.addr, addr) || other.addr == addr));
}


@override
int get hashCode => Object.hash(runtimeType,addr);

@override
String toString() {
  return 'LinkRoute.direct(addr: $addr)';
}


}

/// @nodoc
abstract mixin class $LinkRoute_DirectCopyWith<$Res> implements $LinkRouteCopyWith<$Res> {
  factory $LinkRoute_DirectCopyWith(LinkRoute_Direct value, $Res Function(LinkRoute_Direct) _then) = _$LinkRoute_DirectCopyWithImpl;
@useResult
$Res call({
 String addr
});




}
/// @nodoc
class _$LinkRoute_DirectCopyWithImpl<$Res>
    implements $LinkRoute_DirectCopyWith<$Res> {
  _$LinkRoute_DirectCopyWithImpl(this._self, this._then);

  final LinkRoute_Direct _self;
  final $Res Function(LinkRoute_Direct) _then;

/// Create a copy of LinkRoute
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? addr = null,}) {
  return _then(LinkRoute_Direct(
addr: null == addr ? _self.addr : addr // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class LinkRoute_Relay extends LinkRoute {
  const LinkRoute_Relay({required this.url}): super._();
  

 final  String url;

/// Create a copy of LinkRoute
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$LinkRoute_RelayCopyWith<LinkRoute_Relay> get copyWith => _$LinkRoute_RelayCopyWithImpl<LinkRoute_Relay>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is LinkRoute_Relay&&(identical(other.url, url) || other.url == url));
}


@override
int get hashCode => Object.hash(runtimeType,url);

@override
String toString() {
  return 'LinkRoute.relay(url: $url)';
}


}

/// @nodoc
abstract mixin class $LinkRoute_RelayCopyWith<$Res> implements $LinkRouteCopyWith<$Res> {
  factory $LinkRoute_RelayCopyWith(LinkRoute_Relay value, $Res Function(LinkRoute_Relay) _then) = _$LinkRoute_RelayCopyWithImpl;
@useResult
$Res call({
 String url
});




}
/// @nodoc
class _$LinkRoute_RelayCopyWithImpl<$Res>
    implements $LinkRoute_RelayCopyWith<$Res> {
  _$LinkRoute_RelayCopyWithImpl(this._self, this._then);

  final LinkRoute_Relay _self;
  final $Res Function(LinkRoute_Relay) _then;

/// Create a copy of LinkRoute
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? url = null,}) {
  return _then(LinkRoute_Relay(
url: null == url ? _self.url : url // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

// dart format on
