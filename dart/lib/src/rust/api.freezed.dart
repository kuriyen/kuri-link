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

// dart format on
